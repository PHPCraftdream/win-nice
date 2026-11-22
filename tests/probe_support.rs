//! Probe + win32 support for the harness=false test binaries; each declares
//! this as `mod probe_support`: tests/integration.rs, tests/compat.rs and
//! tests/job_containment.rs. Job-containment probes live here too: breakaway,
//! orphan chain, nested inner job and job-handle audit.
//!
//! See integration.rs for the arg/env contract and the why-harness=false
//! explanation. Depth guard: WNICE_PROBE_DEPTH increments per probe
//! generation; spawns are refused at MAX_PROBE_DEPTH (defense in depth).

use std::io::{BufRead, BufReader, Read, Write};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use windows_sys::Wdk::System::SystemInformation::NtQuerySystemInformation;
use windows_sys::Win32::Foundation::{
    CloseHandle, DuplicateHandle, GetLastError, HANDLE, STILL_ACTIVE,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, JobObjectCpuRateControlInformation,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
    JOBOBJECT_CPU_RATE_CONTROL_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_CPU_RATE_CONTROL_ENABLE, JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, GetExitCodeProcess, GetPriorityClass, GetProcessAffinityMask,
    OpenEventW, OpenProcess, SetEvent, TerminateProcess, WaitForSingleObject, INFINITE,
    PROCESS_DUP_HANDLE, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    SYNCHRONIZATION_SYNCHRONIZE,
};

pub const DETACHED_PROCESS: u32 = 0x0000_0008;
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
pub const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
const INVALID_HANDLE_VALUE: HANDLE = -1isize as HANDLE;
const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;

// ---------------------------------------------------------------- probe ----

pub fn probe_exe() -> PathBuf {
    std::env::current_exe().expect("current_exe")
}

fn probe_args() -> Vec<String> {
    // Extras come in via WNICE_PROBE_ARGS (\u{1f}-separated) — probe argv is
    // kept empty so the harness never option-parses probe arguments.
    match std::env::var("WNICE_PROBE_ARGS") {
        Ok(s) if !s.is_empty() => s.split('\u{1f}').map(|s| s.to_string()).collect(),
        _ => Vec::new(),
    }
}

fn need(args: &[String], i: usize) -> String {
    args.get(i)
        .unwrap_or_else(|| panic!("probe arg {i} missing"))
        .clone()
}

/// Max probe generations allowed. Depth = probe generation: the test harness
/// itself is depth 0, wnice's re-spawn of the probe makes it a depth-1 probe,
/// and each probe's own children increment further. Every spawn that could
/// re-enter probe mode checks this and propagates depth+1 via env. A probe AT
/// depth MAX_PROBE_DEPTH may not spawn anything anymore. The real spawnchain
/// needs 3 generations (root=1 -> midchain=2 -> sleep grandchild=3).
const MAX_PROBE_DEPTH: u32 = 3;

pub fn probe_depth() -> u32 {
    std::env::var("WNICE_PROBE_DEPTH")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

pub fn check_probe_depth() {
    let d = probe_depth();
    if d >= MAX_PROBE_DEPTH {
        panic!("probe recursion guard: refusing to spawn at depth {d} (max {MAX_PROBE_DEPTH})");
    }
}

/// Spawn this test binary as a probe child running `mode` (with extras),
/// with the WNICE_PROBE env pinned so children never recurse into the
/// test loop. WNICE_PROBE_DEPTH increments per generation so daemon/spawnchain
/// children of probes inherit the counter automatically.
pub fn spawn_probe(mode: &str, extras: &[&str], detached: bool) -> std::process::Child {
    check_probe_depth();
    let depth = probe_depth() + 1;
    let mut c = Command::new(probe_exe());
    c.env("WNICE_PROBE", mode)
        .env("WNICE_PROBE_ARGS", extras.join("\u{1f}"))
        .env("WNICE_PROBE_DEPTH", depth.to_string());
    if detached {
        c.creation_flags(DETACHED_PROCESS)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
    } else {
        c.creation_flags(CREATE_NO_WINDOW)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
    }
    c.spawn().expect("probe self-spawn")
}

/// Spawn this test binary as a probe child running `mode` (with extras),
/// capturing its stdout (piped). Used by spawnchain to read midchain's line.
/// Same depth semantics as spawn_probe.
fn spawn_probe_piped(mode: &str, extras: &[&str]) -> std::process::Child {
    check_probe_depth();
    let depth = probe_depth() + 1;
    Command::new(probe_exe())
        .creation_flags(CREATE_NO_WINDOW)
        .env("WNICE_PROBE", mode)
        .env("WNICE_PROBE_ARGS", extras.join("\u{1f}"))
        .env("WNICE_PROBE_DEPTH", depth.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("probe self-spawn")
}

pub fn probe_main(mode: &str) -> ! {
    let rest = probe_args();
    match mode {
        "exit" => {
            let code: i32 = need(&rest, 0).parse().expect("exit code");
            std::process::exit(code);
        }
        "sleep" => {
            let ms: u64 = need(&rest, 0).parse().expect("ms");
            std::thread::sleep(Duration::from_millis(ms));
            std::process::exit(0);
        }
        "echo" => {
            // Print the REAL argv (post-`--` tokens as parsed by wnice's CRT
            // round-trip), not the WNICE_PROBE_ARGS belt-and-suspenders env.
            let argv: Vec<String> = std::env::args().skip(1).collect();
            raw_println(&argv.join("\u{1f}"));
            std::process::exit(0);
        }
        "priority" => {
            unsafe {
                let c = GetPriorityClass(GetCurrentProcess());
                raw_println(&c.to_string());
            }
            std::process::exit(0);
        }
        "affinity" => {
            unsafe {
                let mut pmask: usize = 0;
                let mut smask: usize = 0;
                let ok = GetProcessAffinityMask(GetCurrentProcess(), &mut pmask, &mut smask);
                if ok == 0 {
                    eprintln!("GetProcessAffinityMask failed");
                    std::process::exit(2);
                }
                raw_println(&pmask.to_string());
            }
            std::process::exit(0);
        }
        "jobinfo" => {
            unsafe {
                let mut ext: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                let ok = windows_sys::Win32::System::JobObjects::QueryInformationJobObject(
                    std::ptr::null_mut(),
                    JobObjectExtendedLimitInformation,
                    &mut ext as *mut _ as *mut core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                    std::ptr::null_mut(),
                );
                if ok == 0 {
                    eprintln!("QueryInformationJobObject(extended) failed");
                    std::process::exit(2);
                }
                let mut cpu: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION = std::mem::zeroed();
                let ok = windows_sys::Win32::System::JobObjects::QueryInformationJobObject(
                    std::ptr::null_mut(),
                    JobObjectCpuRateControlInformation,
                    &mut cpu as *mut _ as *mut core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_CPU_RATE_CONTROL_INFORMATION>() as u32,
                    std::ptr::null_mut(),
                );
                if ok == 0 {
                    eprintln!("QueryInformationJobObject(cpu-rate) failed");
                    std::process::exit(2);
                }
                raw_println(&format!(
                    "limitflags={} cpuctrl={} cpurate={}",
                    ext.BasicLimitInformation.LimitFlags, cpu.ControlFlags, cpu.Anonymous.CpuRate
                ));
            }
            std::process::exit(0);
        }
        "allocmem" => {
            let mb: usize = need(&rest, 0).parse().expect("mb");
            let ok = std::panic::catch_unwind(|| {
                let mut v: Vec<u8> = vec![0u8; mb * 1024 * 1024];
                let mut i = 0;
                while i < v.len() {
                    v[i] = 0xAB;
                    i += 4096;
                }
                v.len()
            });
            match ok {
                Ok(_) => {
                    raw_println("ok");
                    std::io::stdout().flush().ok();
                    std::process::exit(0);
                }
                Err(_) => std::process::exit(3),
            }
        }
        "spawnchild" => {
            if let Ok(f) = std::env::var("WNICE_PROBE_OUT") {
                let _ = std::fs::write(
                    format!("{f}.entered"),
                    std::env::var("WNICE_PROBE_ARGS").unwrap_or_default(),
                );
            }
            // Detached sleeper; print its PID then exit immediately.
            let ms = need(&rest, 0);
            let child = spawn_probe("sleep", &[&ms], true);
            raw_println(&child.id().to_string());
            if let Ok(f) = std::env::var("WNICE_PROBE_OUT") {
                let _ = std::fs::write(&f, child.id().to_string());
            }
            std::io::stdout().flush().ok();
            std::process::exit(0);
        }
        "spawnfail" => {
            // Deliberately a plain Command (this IS the spawn under test);
            // env-only so the harness never sees probe argv. Depth-guarded
            // like spawn_probe: this child would re-enter probe mode.
            check_probe_depth();
            let depth = probe_depth() + 1;
            match Command::new(probe_exe())
                .creation_flags(CREATE_NO_WINDOW)
                .env("WNICE_PROBE", "sleep")
                .env("WNICE_PROBE_ARGS", "3000")
                .env("WNICE_PROBE_DEPTH", depth.to_string())
                .spawn()
            {
                Ok(c) => {
                    raw_println(&format!("spawned {}", c.id()));
                }
                Err(_) => {
                    raw_println("spawn-failed");
                }
            }
            std::io::stdout().flush().ok();
            std::process::exit(0);
        }
        "midchain" => {
            // Middle of a REAL 3-generation chain: spawn one sleep grandchild,
            // print "<selfpid> <childpid> <grandpid> <selfprio> <childprio>
            // <grandprio>", then sleep `ms` so the whole chain stays alive for
            // the timeout test. Spawnchain (root) captures this line.
            let ms = need(&rest, 0);
            let grand = spawn_probe("sleep", &[&ms], false);
            raw_println(&format!(
                "{} {} {} {} {} {}",
                std::process::id(),
                grand.id(),
                0,
                unsafe { GetPriorityClass(GetCurrentProcess()) },
                pid_priority(grand.id()).unwrap_or(0),
                0
            ));
            std::io::stdout().flush().ok();
            std::thread::sleep(Duration::from_millis(ms.parse().unwrap_or(5_000)));
            std::process::exit(0);
        }
        "spawnchain" => {
            // Root of a REAL 3-generation chain: spawn ONE midchain probe
            // (which itself spawns the sleep grandchild), read midchain's
            // 6-field line from its piped stdout, and re-print it with root's
            // own pid/prio first — output contract stays
            // "<rootpid> <childpid> <grandpid> <rootprio> <childprio> <grandprio>".
            // Root then sleeps `ms` too so a timeout test kills the whole tree.
            let ms = need(&rest, 0);
            let mut child = spawn_probe_piped("midchain", &[&ms]);
            let line = BufReader::new(child.stdout.take().expect("piped stdout"))
                .lines()
                .next()
                .transpose()
                .ok()
                .flatten()
                .unwrap_or_default();
            // child line: <childpid> <grandpid> 0 <childprio> <grandprio> 0
            let tail: Vec<&str> = line.split_whitespace().collect();
            if tail.len() < 5 {
                eprintln!("midchain line unreadable: {line:?}");
                std::process::exit(2);
            }
            let grandpid: u32 = tail[1].parse().unwrap_or(0);
            let grandprio: u32 = tail[4].parse().unwrap_or(0);
            raw_println(&format!(
                "{} {} {} {} {} {}",
                std::process::id(),
                child.id(),
                grandpid,
                unsafe { GetPriorityClass(GetCurrentProcess()) },
                pid_priority(child.id()).unwrap_or(0),
                grandprio
            ));
            std::io::stdout().flush().ok();
            std::thread::sleep(Duration::from_millis(ms.parse().unwrap_or(5_000)));
            std::process::exit(0);
        }
        "boostmid" => {
            // Middle of a REAL 3-generation chain under booster mode: spawn
            // one sleep grandchild, wait `report_ms` so the asynchronous
            // booster's SetPriorityClass has landed, then print
            // "<childprio> <grandprio> <grandpid>" for boostchain (root) to
            // re-emit. The grandchild sleeps 2x report_ms, so it is alive
            // (and boostable) when the priorities are read and
            // self-terminates afterwards.
            let report_ms: u64 = need(&rest, 0).parse().expect("report ms");
            let grand = spawn_probe("sleep", &[&(2 * report_ms).to_string()], false);
            std::thread::sleep(Duration::from_millis(report_ms));
            raw_println(&format!(
                "{} {} {}",
                unsafe { GetPriorityClass(GetCurrentProcess()) },
                pid_priority(grand.id()).unwrap_or(0),
                grand.id()
            ));
            std::io::stdout().flush().ok();
            std::process::exit(0);
        }
        "boostchain" => {
            // Root of a REAL 3-generation chain under booster mode: spawn ONE
            // boostmid probe (which spawns the sleep grandchild, waits
            // `report_ms` for the booster, and prints
            // "<childprio> <grandprio> <grandpid>"), read that line from its
            // piped stdout, and re-print the standard contract
            // "<rootpid> <childpid> <grandpid> <rootprio> <childprio>
            // <grandprio>" with root's own pid/prio first. Root's prio is
            // read AFTER the line arrives so the booster's root update is
            // visible too. Root exits at once: the grandchild self-terminates
            // after its 2x sleep.
            let report_ms: u64 = need(&rest, 0).parse().expect("report ms");
            let mut child = spawn_probe_piped("boostmid", &[&report_ms.to_string()]);
            let line = BufReader::new(child.stdout.take().expect("piped stdout"))
                .lines()
                .next()
                .transpose()
                .ok()
                .flatten()
                .unwrap_or_default();
            // boostmid line: <childprio> <grandprio> <grandpid>
            let tail: Vec<&str> = line.split_whitespace().collect();
            if tail.len() < 3 {
                eprintln!("boostmid line unreadable: {line:?}");
                std::process::exit(2);
            }
            let childprio: u32 = tail[0].parse().unwrap_or(0);
            let grandprio: u32 = tail[1].parse().unwrap_or(0);
            let grandpid: u32 = tail[2].parse().unwrap_or(0);
            raw_println(&format!(
                "{} {} {} {} {} {}",
                std::process::id(),
                child.id(),
                grandpid,
                unsafe { GetPriorityClass(GetCurrentProcess()) },
                childprio,
                grandprio
            ));
            std::io::stdout().flush().ok();
            std::process::exit(0);
        }
        "guardedspawn" => {
            // Runs at WNICE_PROBE_DEPTH = MAX_PROBE_DEPTH: the inner spawn must
            // be refused by the recursion guard (panic caught here).
            let r = std::panic::catch_unwind(|| spawn_probe("sleep", &["100"], false));
            match r {
                Ok(c) => raw_println(&format!("spawned {}", c.id())),
                Err(e) => {
                    let msg = e
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                        .unwrap_or_default();
                    if msg.contains("probe recursion guard") {
                        raw_println("refused");
                    } else {
                        raw_println("spawned other-panic");
                    }
                }
            }
            std::io::stdout().flush().ok();
            std::process::exit(0);
        }
        "parentpid" => {
            raw_println(&parent_pid(std::process::id()).unwrap_or(0).to_string());
            std::process::exit(0);
        }
        "wait-eof" => {
            // Alive on its stdin pipe, not a sleep: returns when the owner
            // closes the write end (or dies).
            let mut sink = Vec::new();
            let _ = std::io::stdin().read_to_end(&mut sink);
            std::process::exit(0);
        }
        "wait-event" => {
            // Same, on a named event the owner signals.
            let name = need(&rest, 0);
            let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            let h = unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, 0, wide.as_ptr()) };
            if h.is_null() {
                eprintln!("OpenEventW failed {}", unsafe { GetLastError() });
                std::process::exit(3);
            }
            unsafe {
                WaitForSingleObject(h, INFINITE);
            }
            std::process::exit(0);
        }
        "orphan-parent" => {
            // Chain middle: spawn grandchild, hand its handle to root and test, exit.
            let event = std::env::var("WNICE_TEST_EVENT").expect("WNICE_TEST_EVENT");
            let test_pid: u32 = std::env::var("WNICE_TEST_PID")
                .expect("WNICE_TEST_PID")
                .parse()
                .expect("test pid");
            check_probe_depth();
            let grand = Command::new(probe_exe())
                .creation_flags(CREATE_NO_WINDOW)
                .env("WNICE_PROBE", "wait-event")
                .env("WNICE_PROBE_ARGS", &event)
                .env("WNICE_PROBE_DEPTH", (probe_depth() + 1).to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("grandchild spawn");
            let raw = grand.as_raw_handle() as HANDLE;
            let in_root = match duplicate_into_parent(raw) {
                Ok(v) => v,
                Err(c) => {
                    raw_println(&format!("duplicate-failed {c}"));
                    std::process::exit(3);
                }
            };
            let in_test = match duplicate_into(test_pid, raw) {
                Ok(v) => v,
                Err(c) => {
                    raw_println(&format!("duplicate-failed {c}"));
                    std::process::exit(3);
                }
            };
            raw_println(&format!(
                "grandpid={} in_root={in_root} in_test={in_test}",
                grand.id()
            ));
            std::io::stdout().flush().ok();
            std::process::exit(0);
        }
        "orphan-root" => {
            // Chain root: learn grandchild id from middle, await its real exit.
            let mut mid = spawn_probe_piped("orphan-parent", &[]);
            let line = BufReader::new(mid.stdout.take().expect("piped stdout"))
                .lines()
                .next()
                .transpose()
                .ok()
                .flatten()
                .unwrap_or_default();
            let exit = mid.wait().ok().and_then(|s| s.code()).unwrap_or(-1);
            let get = |key: &str| -> i64 {
                line.split_whitespace()
                    .find_map(|t| t.strip_prefix(key)?.strip_prefix('=')?.parse().ok())
                    .unwrap_or(-1)
            };
            let in_root = get("in_root") as usize as HANDLE;
            let injob = u8::from(process_in_own_job(in_root) == Some(true));
            let alive = u8::from(handle_exit_code(in_root) == Some(STILL_ACTIVE as u32));
            raw_println(&format!(
                "ready intermediate_exit={exit} injob={injob} alive={alive} test_handle={} grandpid={}",
                get("in_test"),
                get("grandpid")
            ));
            std::io::stdout().flush().ok();
            let mut sink = Vec::new();
            let _ = std::io::stdin().read_to_end(&mut sink);
            std::process::exit(0);
        }
        "innerjob" => {
            // Nest in a loose job (no flags, 100% CPU); kids block on stdin pipe.
            let attempts: u32 = need(&rest, 0).parse().expect("attempts");
            let ext: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            let mut cpu: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION = unsafe { std::mem::zeroed() };
            cpu.ControlFlags =
                JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP;
            cpu.Anonymous.CpuRate = 10_000;
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                let a = SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &ext as *const _ as *const core::ffi::c_void,
                    std::mem::size_of_val(&ext) as u32,
                );
                let b = SetInformationJobObject(
                    job,
                    JobObjectCpuRateControlInformation,
                    &cpu as *const _ as *const core::ffi::c_void,
                    std::mem::size_of_val(&cpu) as u32,
                );
                if job.is_null() || a == 0 || b == 0 {
                    eprintln!("inner job setup failed: {}", GetLastError());
                    std::process::exit(3);
                }
                if AssignProcessToJobObject(job, GetCurrentProcess()) == 0 {
                    raw_println(&format!("assign-failed {}", GetLastError()));
                    std::process::exit(3);
                }
            }
            let rate = unsafe { cpu.Anonymous.CpuRate };
            raw_println(&format!(
                "inner limitflags={} cpuctrl={} cpurate={rate}",
                ext.BasicLimitInformation.LimitFlags, cpu.ControlFlags
            ));
            let mut kids = Vec::new();
            let mut failed = None;
            for _ in 0..attempts {
                check_probe_depth();
                match Command::new(probe_exe())
                    .creation_flags(CREATE_NO_WINDOW)
                    .env("WNICE_PROBE", "wait-eof")
                    .env("WNICE_PROBE_DEPTH", (probe_depth() + 1).to_string())
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                {
                    Ok(mut c) => {
                        let sin = c.stdin.take();
                        kids.push((c, sin));
                    }
                    Err(e) => {
                        failed = Some(e.raw_os_error().unwrap_or(-1));
                        break;
                    }
                }
            }
            let mut alive = 0;
            for (c, _) in kids.iter_mut() {
                if matches!(c.try_wait(), Ok(None)) {
                    alive += 1;
                }
            }
            let failed = failed.map_or_else(|| "none".to_string(), |c| c.to_string());
            raw_println(&format!(
                "spawned={} alive={alive} failed={failed}",
                kids.len()
            ));
            std::io::stdout().flush().ok();
            for (c, sin) in &mut kids {
                drop(sin.take());
                let _ = c.wait();
            }
            std::process::exit(0);
        }
        "breakaway" => {
            // This spawn IS the subject: inside a job that grants no
            // breakaway the kernel must refuse it.
            check_probe_depth();
            // Fingerprint the enclosing job FIRST, so a refusal caused by
            // something other than this job cannot read as the guarantee.
            unsafe {
                let mut ext: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                let ok = windows_sys::Win32::System::JobObjects::QueryInformationJobObject(
                    std::ptr::null_mut(),
                    JobObjectExtendedLimitInformation,
                    &mut ext as *mut _ as *mut core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                    std::ptr::null_mut(),
                );
                if ok == 0 {
                    eprintln!("QueryInformationJobObject(extended) failed");
                    std::process::exit(3);
                }
                let mut cpu: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION = std::mem::zeroed();
                let ok = windows_sys::Win32::System::JobObjects::QueryInformationJobObject(
                    std::ptr::null_mut(),
                    JobObjectCpuRateControlInformation,
                    &mut cpu as *mut _ as *mut core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_CPU_RATE_CONTROL_INFORMATION>() as u32,
                    std::ptr::null_mut(),
                );
                if ok == 0 {
                    eprintln!("QueryInformationJobObject(cpu-rate) failed");
                    std::process::exit(3);
                }
                raw_println(&format!(
                    "job limitflags={} cpurate={}",
                    ext.BasicLimitInformation.LimitFlags, cpu.Anonymous.CpuRate
                ));
            }
            match Command::new(probe_exe())
                .creation_flags(CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB)
                .env("WNICE_PROBE", "sleep")
                .env("WNICE_PROBE_ARGS", "3000")
                .env("WNICE_PROBE_DEPTH", (probe_depth() + 1).to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(mut child) => {
                    let pid = child.id();
                    let _ = child.kill();
                    let _ = child.wait();
                    raw_println(&format!("spawned {pid}"));
                }
                // Win32 error as i32; 5 = ERROR_ACCESS_DENIED.
                Err(e) => raw_println(&format!("refused {}", e.raw_os_error().unwrap_or(-1))),
            }
            std::io::stdout().flush().ok();
            std::process::exit(0);
        }
        "handleaudit" => {
            // Built-in control: three job handles are created here, so the
            // kernel's own handle table must report exactly 3 Job-typed
            // handles for this process. An inherited job handle shows up as 4,
            // a broken enumeration as anything else.
            let mut own: Vec<HANDLE> = Vec::new();
            for _ in 0..3 {
                let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
                if job.is_null() {
                    eprintln!("CreateJobObjectW failed");
                    std::process::exit(3);
                }
                // Handles stay open for the life of the process.
                own.push(job);
            }
            match audit_job_handles(&own) {
                Ok((type_index, count, total, injob)) => {
                    raw_println(&format!(
                        "jobhandles={count} own={} injob={injob} jobtype={type_index} handles={total}",
                        own.len()
                    ));
                    std::io::stdout().flush().ok();
                    std::process::exit(0);
                }
                Err(e) => {
                    eprintln!("handle audit: {e}");
                    std::process::exit(3);
                }
            }
        }
        other => {
            eprintln!("unknown probe mode {other}");
            std::process::exit(2);
        }
    }
}

// ------------------------------------------------------------ handle audit

// SystemExtendedHandleInformation; windows-sys 0.59 exports no constant.
const SYSTEM_EXTENDED_HANDLE_INFORMATION: i32 = 64;
const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004u32 as i32;

/// This process's handle table, straight from the kernel: `(HandleValue, ObjectTypeIndex)`.
fn own_handles() -> Result<Vec<(usize, u16)>, String> {
    let mut len: usize = 1 << 20;
    for _ in 0..6 {
        let mut b = vec![0u8; len];
        let mut ret_len: u32 = 0;
        let status = unsafe {
            NtQuerySystemInformation(
                SYSTEM_EXTENDED_HANDLE_INFORMATION,
                b.as_mut_ptr() as *mut core::ffi::c_void,
                len as u32,
                &mut ret_len,
            )
        };
        // A success status is not enough: the buffer must hold every entry the
        // header claims, or the table is incomplete and must never be trusted.
        if status == 0 && ret_len as usize <= len && whole_snapshot(&b, len) {
            return Ok(parse_handles(&b));
        }
        if status != 0 && status != STATUS_INFO_LENGTH_MISMATCH {
            return Err(format!("NtQuerySystemInformation: NTSTATUS {status:#010x}"));
        }
        len = (ret_len as usize).max(len * 2);
    }
    Err(format!(
        "handle table did not fit in {len} bytes after 6 attempts"
    ))
}

/// True when `b` holds every entry the snapshot header claims.
fn whole_snapshot(b: &[u8], len: usize) -> bool {
    let w = std::mem::size_of::<usize>();
    len >= 2 * w && b.len() >= 2 * w && 2 * w + read_at::<usize>(b, 0) * (3 * w + 16) <= len
}

/// `(HandleValue, ObjectTypeIndex)` of every handle this process owns.
fn parse_handles(b: &[u8]) -> Vec<(usize, u16)> {
    let w = std::mem::size_of::<usize>();
    let pid = std::process::id() as usize;
    let n = read_at::<usize>(b, 0);
    let mut out = Vec::new();
    for i in 0..n {
        let base = 2 * w + i * (3 * w + 16);
        if read_at::<usize>(b, base + w) == pid {
            out.push((
                read_at::<usize>(b, base + 2 * w),
                read_at::<u16>(b, base + 3 * w + 6),
            ));
        }
    }
    out
}

fn read_at<T: Copy>(buf: &[u8], off: usize) -> T {
    unsafe { std::ptr::read_unaligned(buf.as_ptr().add(off) as *const T) }
}

/// `(job type index, Job-typed handles owned, total handles owned, ...)`.
fn audit_job_handles(own: &[HANDLE]) -> Result<(u16, usize, usize, usize), String> {
    let snaps = own_handles()?;
    let pid = std::process::id();
    if snaps.is_empty() {
        return Err(format!(
            "handle table lists no handles for pid {pid} (enumeration broken)"
        ));
    }
    let first = *own
        .first()
        .ok_or_else(|| "audit called with no own handles".to_string())? as usize;
    let type_index = snaps
        .iter()
        .find(|(h, _)| *h == first)
        .map(|(_, t)| *t)
        .ok_or_else(|| {
            format!("own job handle {first:#x} missing from own handle table (pid {pid})")
        })?;
    // EVERY own handle must appear exactly once, with that same type: a
    // partial or duplicated view must not read as "no inherited job handle".
    for h in own {
        let h = *h as usize;
        let seen: Vec<u16> = snaps
            .iter()
            .filter(|(v, _)| *v == h)
            .map(|(_, t)| *t)
            .collect();
        if seen.len() != 1 || seen[0] != type_index {
            return Err(format!(
                "own job handle {h:#x} observed {seen:?} in own handle table \
                 (pid {pid}), expected exactly one entry of type {type_index}"
            ));
        }
    }
    let count = snaps.iter().filter(|(_, t)| *t == type_index).count();
    let injob = snaps
        .iter()
        .filter(|(v, t)| *t == type_index && in_own_job(*v as HANDLE) == Some(true))
        .count();
    Ok((type_index, count, snaps.len(), injob))
}

// ---------------------------------------------------- handle / event helpers

/// Manual-reset event the test owns and signals to release helpers that block
/// on it (never a sleep). NULL on failure.
pub fn create_event(name: &str) -> HANDLE {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe { CreateEventW(std::ptr::null(), 1, 0, wide.as_ptr()) }
}

#[allow(clippy::not_unsafe_ptr_arg_deref)] // these take an already-open handle
pub fn set_event(h: HANDLE) {
    unsafe {
        SetEvent(h);
    }
}

#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn wait_handle(h: HANDLE, ms: u32) -> u32 {
    unsafe { WaitForSingleObject(h, ms) }
}

/// Exit code of the process behind `h`; None when the query itself fails.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn handle_exit_code(h: HANDLE) -> Option<u32> {
    let mut code: u32 = 0;
    (unsafe { GetExitCodeProcess(h, &mut code) } != 0).then_some(code)
}

/// Duplicate `h` into process `pid` with SYNCHRONIZE|PROCESS_QUERY_LIMITED_INFORMATION;
/// returns the handle value valid in the target process.
#[allow(dead_code)] // wired into a probe in the next chunk
fn duplicate_into(pid: u32, h: HANDLE) -> Result<usize, u32> {
    unsafe {
        let target = OpenProcess(PROCESS_DUP_HANDLE, 0, pid);
        if target.is_null() {
            return Err(GetLastError());
        }
        let mut out: HANDLE = std::ptr::null_mut();
        // SYNCHRONIZE only exists as SYNCHRONIZATION_SYNCHRONIZE here.
        let ok = DuplicateHandle(
            GetCurrentProcess(),
            h,
            target,
            &mut out,
            SYNCHRONIZATION_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            0,
        );
        let err = GetLastError(); // read before CloseHandle can overwrite it
        CloseHandle(target);
        if ok == 0 {
            return Err(err);
        }
        Ok(out as usize)
    }
}

/// Same, into this process's parent (the chain root).
#[allow(dead_code)] // wired into a probe in the next chunk
fn duplicate_into_parent(h: HANDLE) -> Result<usize, u32> {
    match parent_pid(std::process::id()) {
        Some(pid) => duplicate_into(pid, h),
        None => Err(unsafe { GetLastError() }),
    }
}

/// True when this process is a member of the job behind `h_job`.
#[allow(dead_code)] // wired into a probe in the next chunk
fn in_own_job(h_job: HANDLE) -> Option<bool> {
    let mut r: i32 = 0;
    (unsafe { IsProcessInJob(GetCurrentProcess(), h_job, &mut r) } != 0).then_some(r != 0)
}

/// True when the process behind `h_process` is in the job of THIS process.
fn process_in_own_job(h_process: HANDLE) -> Option<bool> {
    let mut r: i32 = 0;
    let ok = unsafe { IsProcessInJob(h_process, std::ptr::null_mut(), &mut r) };
    (ok != 0).then_some(r != 0)
}

/// Owns a survivor process through its own handle: the numeric PID is used
/// only once, to open it, so a later guard can never hit a reused PID.
pub struct SurvivorGuard(HANDLE);

impl SurvivorGuard {
    /// Opens `pid` for query+terminate; a null handle means "already gone".
    pub fn open(pid: u32) -> SurvivorGuard {
        let access = PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE;
        let h = unsafe { OpenProcess(access, 0, pid) };
        SurvivorGuard(h)
    }

    /// True while the process behind the retained handle is still running.
    pub fn alive(&self) -> bool {
        handle_exit_code(self.0) == Some(STILL_ACTIVE as u32)
    }

    /// Terminate through the retained handle; false when it is already gone.
    pub fn kill(&self) -> bool {
        if !self.alive() {
            return false;
        }
        unsafe { TerminateProcess(self.0, 1) != 0 }
    }
}

impl Drop for SurvivorGuard {
    fn drop(&mut self) {
        if self.alive() {
            unsafe {
                TerminateProcess(self.0, 1);
            }
        }
        if !self.0.is_null() {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

// ------------------------------------------------------------- win32 helpers

pub fn pid_priority(pid: u32) -> Option<u32> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let p = GetPriorityClass(h);
        CloseHandle(h);
        Some(p)
    }
}

/// True when the process is alive (exit code != STILL_ACTIVE means dead).
pub fn pid_alive(pid: u32) -> bool {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return false; // gone (or inaccessible)
        }
        let mut code: u32 = 0;
        let ok = GetExitCodeProcess(h, &mut code);
        CloseHandle(h);
        ok != 0 && code == STILL_ACTIVE as u32
    }
}

/// True when the test harness itself already runs inside a non-default job
/// object (CI/sandbox runners do this). A parent job with a priority-class
/// limit overrides any priority wnice sets, so priority tests skip there.
pub fn running_in_nested_job() -> bool {
    unsafe {
        let mut r: i32 = 0;
        windows_sys::Win32::System::JobObjects::IsProcessInJob(
            GetCurrentProcess(),
            std::ptr::null_mut(),
            &mut r,
        );
        r != 0
    }
}

/// True when the surrounding sandbox job forces IDLE priority class on the
/// whole tree (guarded.ps1 does this). wnice cannot raise child priorities
/// out of a job cap, so priority expectations are untestable there.
pub fn guard_job_forces_idle() -> bool {
    running_in_nested_job() && pid_priority(std::process::id()) == Some(0x40)
}

/// Kill by explicit PID of a process this test started. Returns success.
pub fn kill_pid(pid: u32) -> bool {
    unsafe {
        let h = OpenProcess(
            PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        );
        if h.is_null() {
            return false;
        }
        let ok = TerminateProcess(h, 1);
        CloseHandle(h);
        ok != 0
    }
}

pub fn parent_pid(pid: u32) -> Option<u32> {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut result = None;
        if Process32FirstW(snap, &mut e) != 0 {
            loop {
                if e.th32ProcessID == pid {
                    result = Some(e.th32ParentProcessID);
                    break;
                }
                if Process32NextW(snap, &mut e) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
        result
    }
}

/// Print to the REAL process stdout. Probe children inherit wnice's stdout
/// pipe; raw_println writes straight to the std handle without any buffered
/// wrapper in between (flush semantics are exact even right before exit()).
pub fn raw_println(msg: &str) {
    use std::os::windows::io::FromRawHandle;
    unsafe {
        let h = windows_sys::Win32::System::Console::GetStdHandle(STD_OUTPUT_HANDLE);
        let mut f = std::fs::File::from_raw_handle(h);
        let _ = writeln!(f, "{msg}");
        let _ = f.flush();
        std::mem::forget(f); // do not close the shared std handle
    }
}
