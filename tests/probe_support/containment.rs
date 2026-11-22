//! Job-containment probes: part of tests/probe_support, included by the
//! harness=false suites.

use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};

use windows_sys::Wdk::System::SystemInformation::NtQuerySystemInformation;
use windows_sys::Win32::Foundation::{CloseHandle, DuplicateHandle, GetLastError, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, JobObjectCpuRateControlInformation,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
    JOBOBJECT_CPU_RATE_CONTROL_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_CPU_RATE_CONTROL_ENABLE, JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, PROCESS_DUP_HANDLE, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    SYNCHRONIZATION_SYNCHRONIZE,
};

use super::*;

/// Kills and reaps its children on drop, so no path leaves a blocked child behind.
struct Kids(Vec<(std::process::Child, Option<std::process::ChildStdin>)>);

impl Drop for Kids {
    fn drop(&mut self) {
        for (c, sin) in &mut self.0 {
            drop(sin.take());
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// Runs a containment probe mode; None when the mode is not one of ours.
pub fn try_run(mode: &str) -> Option<i32> {
    let rest = probe_args();
    match mode {
        "wait-eof" => {
            // Alive on its stdin pipe, not a sleep: returns when the owner
            // closes the write end (or dies).
            let mut sink = Vec::new();
            let _ = std::io::stdin().read_to_end(&mut sink);
            std::process::exit(0);
        }
        "ready-wait-eof" => {
            // Readiness on the inherited stdout pipe, then stay alive on stdin
            // until the owner closes it: no sleep, no timer, no polling.
            raw_println("ready");
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
            let mut kids = Kids(Vec::new());
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
                        kids.0.push((c, sin));
                    }
                    Err(e) => {
                        failed = Some(e.raw_os_error().unwrap_or(-1));
                        break;
                    }
                }
            }
            let mut alive = 0;
            for (c, _) in kids.0.iter_mut() {
                if matches!(c.try_wait(), Ok(None)) {
                    alive += 1;
                }
            }
            let failed = failed.map_or_else(|| "none".to_string(), |c| c.to_string());
            raw_println(&format!(
                "spawned={} alive={alive} failed={failed}",
                kids.0.len()
            ));
            std::io::stdout().flush().ok();
            // `exit` skips destructors: release the guard explicitly so the
            // children are terminated and reaped on this path too.
            drop(kids);
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

        _ => None,
    }
}

// ------------------------------------------------------------ handle audit

// SystemExtendedHandleInformation; windows-sys 0.59 exports no constant.
const SYSTEM_EXTENDED_HANDLE_INFORMATION: i32 = 64;
const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004u32 as i32;

/// This process's handle table, straight from the kernel: `(HandleValue, ObjectTypeIndex)`.
///
/// The snapshot is only trusted when it is complete: the header must claim no
/// more entries than the bytes the kernel reported writing.
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
        // header claims. ReturnLength is what the kernel reports writing; when
        // it is absent the snapshot is unusable, and capacity is never a
        // substitute for it.
        if status == 0 {
            let written = ret_len as usize;
            if written > 0 && written <= len && whole_snapshot(&b, written) {
                return Ok(parse_handles(&b));
            }
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

/// True when `b` holds every entry the snapshot header claims, counting only
/// the `written` bytes the kernel reported having filled.
fn whole_snapshot(b: &[u8], written: usize) -> bool {
    let w = std::mem::size_of::<usize>();
    let header = match 2usize.checked_mul(w) {
        Some(v) => v,
        None => return false,
    };
    if b.len() < header || written < header || written > b.len() {
        return false;
    }
    let n = read_at::<usize>(b, 0);
    match n
        .checked_mul(3 * w + 16)
        .and_then(|v| v.checked_add(header))
    {
        Some(need) => need <= written,
        None => false,
    }
}

/// `(HandleValue, ObjectTypeIndex)` of every handle this process owns.
///
/// Iterates only within the bound validated by `whole_snapshot`, so a short
/// snapshot can never drive a read past the end of `b`.
fn parse_handles(b: &[u8]) -> Vec<(usize, u16)> {
    let w = std::mem::size_of::<usize>();
    let pid = std::process::id() as usize;
    let header = 2 * w;
    let stride = 3 * w + 16;
    let n = read_at::<usize>(b, 0);
    let mut out = Vec::new();
    for i in 0..n {
        let Some(base) = i.checked_mul(stride).and_then(|v| v.checked_add(header)) else {
            break;
        };
        if base + stride > b.len() {
            break;
        }
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

/// Duplicate `h` into process `pid` with SYNCHRONIZE|QUERY_LIMITED|TERMINATE, so the
/// owner can both observe and terminate; returns the handle valid in the target.
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
            SYNCHRONIZATION_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE,
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

/// Hands this process's own handle to the test that owns WNICE_TEST_PID, by
/// writing the target-side handle value to WNICE_TEST_HANDLE_OUT (temp file +
/// rename). Silent only when either variable is unset (not asked to publish);
/// once asked, a failure is reported and exits so it cannot be missed.
pub fn hand_off_self() {
    let (Ok(raw_pid), Ok(path)) = (
        std::env::var("WNICE_TEST_PID"),
        std::env::var("WNICE_TEST_HANDLE_OUT"),
    ) else {
        return; // not asked to publish
    };
    let pid = match raw_pid.parse::<u32>() {
        Ok(p) => p,
        Err(e) => publish_failed(&format!("WNICE_TEST_PID {raw_pid:?}: {e}")),
    };
    let value = match duplicate_into(pid, unsafe { GetCurrentProcess() }) {
        Ok(v) => v,
        Err(code) => publish_failed(&format!(
            "DuplicateHandle into pid {pid}: Win32 error {code}"
        )),
    };
    let tmp = format!("{path}.tmp");
    if let Err(e) = std::fs::write(&tmp, value.to_string()) {
        publish_failed(&format!("cannot write {tmp}: {e}"));
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        publish_failed(&format!("cannot publish {path}: {e}"));
    }
}

/// A survivor that was told to publish its handle must not continue without it.
fn publish_failed(why: &str) -> ! {
    eprintln!("probe: handle hand-off failed: {why}");
    std::process::exit(7);
}

/// True when this process is a member of the job behind `h_job`.
fn in_own_job(h_job: HANDLE) -> Option<bool> {
    let mut r: i32 = 0;
    (unsafe { IsProcessInJob(GetCurrentProcess(), h_job, &mut r) } != 0).then_some(r != 0)
}
