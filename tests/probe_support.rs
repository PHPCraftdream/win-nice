//! Probe + win32 support for the harness=false integration test binary
//! (tests/integration.rs includes this as `mod probe_support`).
//!
//! See integration.rs for the arg/env contract and the why-harness=false
//! explanation. Depth guard: WNICE_PROBE_DEPTH increments per probe
//! generation; spawns are refused at MAX_PROBE_DEPTH (defense in depth).

use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, STILL_ACTIVE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::JobObjects::{
    JobObjectCpuRateControlInformation, JobObjectExtendedLimitInformation,
    JOBOBJECT_CPU_RATE_CONTROL_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, GetPriorityClass, GetProcessAffinityMask, OpenProcess,
    TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
};

pub const DETACHED_PROCESS: u32 = 0x0000_0008;
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
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
        other => {
            eprintln!("unknown probe mode {other}");
            std::process::exit(2);
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
