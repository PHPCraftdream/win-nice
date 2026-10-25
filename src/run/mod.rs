//! The `run` engine: port of `CapsLauncher.Run()` in `bin/caps.ps1`.
//!
//! Sequence mirrors the C# exactly: compose limits -> create job -> apply
//! extended limits -> (optional) CPU-rate cap -> launch suspended -> assign to
//! job -> resume -> absolute-FILETIME waitable-timer wait -> tie-break by
//! kernel exit time -> timeout kill (exit 124) or exit-code propagation with a
//! best-effort release of the kill-on-close guard.
//!
//! Priority probing design (the two-step variant): when `cfg.priority` is
//! set, the extended limits are applied FIRST WITHOUT the priority flag/class,
//! then a second apply WITH them probes whether the job accepts tree-wide
//! priority. A 1314 (ERROR_PRIVILEGE_NOT_HELD) keeps the no-priority version,
//! emits one stderr note, and ORs the priority class into CreateProcess flags
//! so the root process still gets it (realtime downgrades to High = 0x80, as
//! in the existing tools). Any other error from the with-priority probe is a
//! real failure. Cost: two `SetInformationJobObject` calls when priority is
//! requested, one otherwise — cheap, and it makes the "job refused priority"
//! case unambiguous (the no-priority baseline is already live, so the probe
//! result alone decides).

use crate::launch;
use crate::limits;
use crate::win32_err;
use crate::{Config, Sys};

/// FILETIME epoch offset: 100ns units between 1601-01-01 and 1970-01-01.
const FILETIME_EPOCH_OFFSET_100NS: u64 = 116_444_736_000_000_000;

/// Current UTC time as a 64-bit FILETIME (same clock/epoch the waitable
/// timer's absolute due time uses).
fn now_filetime() -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before 1970");
    let ticks = now.as_secs() * 10_000_000 + u64::from(now.subsec_nanos()) / 100;
    (ticks + FILETIME_EPOCH_OFFSET_100NS) as i64
}

/// Human-facing timeout length for the timeout message. `Config` stores only
/// whole milliseconds, so derive the display: integer seconds when exact,
/// otherwise seconds with up to 3 decimals, trailing zeros trimmed
/// (2000 -> "2s", 2500 -> "2.5s", 1234 -> "1.234s").
fn timeout_display(ms: u32) -> String {
    if ms.is_multiple_of(1000) {
        return format!("{}s", ms / 1000);
    }
    let s = format!("{:.3}", ms as f64 / 1000.0);
    format!("{}s", s.trim_end_matches('0').trim_end_matches('.'))
}

/// Append the "TerminateProcess also failed" suffix when the best-effort kill
/// on an error path itself fails (exact caps.ps1 message format).
fn append_kill_failure(msg: &mut String, code: u32) {
    msg.push_str("; TerminateProcess also failed: ");
    msg.push_str(&code.to_string());
}

pub fn run(cfg: &Config, sys: &dyn Sys) -> i32 {
    run_with_writer(cfg, sys, &mut |msg| eprintln!("{msg}"))
}

/// Core of [`run`] with an injectable stderr writer, for tests.
pub fn run_with_writer(cfg: &Config, sys: &dyn Sys, err: &mut dyn FnMut(&str)) -> i32 {
    run_with_writer_in(cfg, sys, &launch::path_dirs(), err)
}

/// Like [`run_with_writer`] but with explicit search directories for bare
/// command names (the current directory is NEVER searched). Tests inject a
/// fake list so they don't depend on the real `PATH`.
pub fn run_with_writer_in(
    cfg: &Config,
    sys: &dyn Sys,
    dirs: &[String],
    err: &mut dyn FnMut(&str),
) -> i32 {
    let limits = match limits::compose(cfg, sys) {
        Ok(l) => l,
        Err(msg) => {
            err(&msg);
            return 1;
        }
    };

    let job = match sys.create_job_object() {
        Ok(j) => j,
        Err(code) => {
            err(&win32_err("CreateJobObject", code));
            return 1;
        }
    };

    // Priority probing: apply WITHOUT priority first, then probe WITH it.
    // The `applied` limits value (priority included or not) is what gets
    // released on the success path.
    let mut creation_priority: Option<u32> = None;
    let mut applied = limits;
    applied.flags &= !crate::JOB_OBJECT_LIMIT_PRIORITY_CLASS;
    applied.priority_class = 0;
    let applied_info = limits::extended_limit_info(&applied, true);
    if let Err(code) = sys.set_information_job_object(
        &job,
        crate::JobObjectExtendedLimitInformation,
        &limits::extended_bytes(&applied_info),
    ) {
        err(&win32_err("SetInformationJobObject", code));
        return 1;
    }
    if let Some(prio) = cfg.priority {
        let probe_info = limits::extended_limit_info(&limits, true);
        match sys.set_information_job_object(
            &job,
            crate::JobObjectExtendedLimitInformation,
            &limits::extended_bytes(&probe_info),
        ) {
            Ok(()) => {
                // The with-priority version is what's live on the job now.
                applied = limits;
                let class = if prio == crate::PriorityClass::Realtime {
                    // Existing tools' convention: realtime refused by the job
                    // silently becomes High.
                    crate::PriorityClass::High.to_u32()
                } else {
                    prio.to_u32()
                };
                creation_priority = Some(class);
            }
            Err(crate::ERROR_PRIVILEGE_NOT_HELD) => {
                err(&format!(
                    "wnice: note: {} priority applies to the root process only (the job refused tree-wide priority: Win32 error 1314)",
                    format!("{prio:?}").to_lowercase()
                ));
                creation_priority = Some(if prio == crate::PriorityClass::Realtime {
                    crate::PriorityClass::High.to_u32()
                } else {
                    prio.to_u32()
                });
            }
            Err(code) => {
                err(&win32_err("SetInformationJobObject", code));
                return 1;
            }
        }
    }

    if let Some(p) = applied.cpu_rate {
        let cpu_info = limits::cpu_rate_info(p);
        if let Err(code) = sys.set_information_job_object(
            &job,
            crate::JobObjectCpuRateControlInformation,
            &limits::cpu_rate_bytes(&cpu_info),
        ) {
            err(&win32_err("SetInformationJobObject", code));
            return 1;
        }
    }

    if cfg.command.is_empty() {
        err("no command given");
        return 1;
    }

    // Bare command names are resolved ONLY against the explicit dirs (PATH
    // with cwd-like entries dropped). Resolution failure is exit 127.
    let argv0_abs = match launch::resolve_command(dirs, &cfg.command[0]) {
        None => {
            err(&format!(
                "wnice: '{}': command not found (searched PATH; the current directory is never searched)",
                cfg.command[0]
            ));
            return 127;
        }
        Some(launch::Resolved::Direct(abs)) => abs,
        Some(launch::Resolved::Shell(abs)) => abs,
    };
    let shell_target = launch::is_bat_or_cmd(&argv0_abs);

    // Full argv with argv[0] rewritten to the resolved absolute path.
    let argv: Vec<&str> = std::iter::once(argv0_abs.as_str())
        .chain(cfg.command[1..].iter().map(|s| s.as_str()))
        .collect();
    let direct_cmdline = launch::build_argv_command_line(&argv);
    let cmd_exe = format!("{}\\cmd.exe", sys.system_directory());
    let shell_cmdline =
        launch::shell_command_line(&cmd_exe, &launch::build_cmd_fallback_command_line(&argv));

    let creation_flags = crate::CREATE_SUSPENDED | creation_priority.unwrap_or(0);

    let mut child = None;
    if !shell_target {
        // Direct launch: resolved exe as lpApplicationName.
        if let Ok(c) = sys.create_process(Some(&argv0_abs), &direct_cmdline, creation_flags) {
            child = Some(c);
        }
    }
    if child.is_none() {
        // cmd.exe fallback time (always for resolved .bat/.cmd targets): a
        // literal `%` could be expanded by cmd even across separate arguments
        // — refuse loudly instead (caps.ps1 throws; here the caller's writer
        // reports it).
        if launch::contains_percent(&argv) {
            err(&format!("wnice: {}", launch::PERCENT_REFUSAL_MESSAGE));
            return 1;
        }
        match sys.create_process(None, &shell_cmdline, creation_flags) {
            Ok(c) => child = Some(c),
            Err(code) => {
                err(&win32_err("CreateProcess", code));
                return 1;
            }
        }
    }
    let child = child.expect("child set by one of the branches above");

    if let Err(code) = sys.assign_process_to_job_object(&job, &child.process) {
        // Can't guarantee the limits - kill instead of letting it run unmanaged.
        let mut msg = win32_err("AssignProcessToJobObject", code);
        if let Err(kill_code) = sys.terminate_process(&child.process, 1) {
            append_kill_failure(&mut msg, kill_code);
        }
        err(&msg);
        return 1;
    }

    if let Err(code) = sys.resume_thread(&child.thread) {
        // Still suspended - an unbounded wait below would hang forever.
        let mut msg = win32_err("ResumeThread", code);
        if let Err(kill_code) = sys.terminate_process(&child.process, 1) {
            append_kill_failure(&mut msg, kill_code);
        }
        err(&msg);
        return 1;
    }

    // Absolute deadline, armed into a one-shot manual-reset waitable timer and
    // waited on TOGETHER with the process handle (see caps.ps1 for why this
    // must never be a relative wait: suspend/resume counting).
    let mut due_time: i64 = 0;
    let mut timer = None;
    if let Some(ms) = cfg.timeout_ms {
        due_time = now_filetime() + i64::from(ms) * 10_000;
        let t = match sys.create_waitable_timer() {
            Ok(t) => t,
            Err(code) => {
                // Child already running: nothing is terminated here — the job
                // handle's Drop below carries the still-armed
                // KILL_ON_JOB_CLOSE flag, which is the documented backstop
                // that kills the whole tree when this wrapper exits.
                err(&win32_err("CreateWaitableTimer", code));
                return 1;
            }
        };
        if let Err(code) = sys.set_waitable_timer(&t, due_time) {
            err(&win32_err("SetWaitableTimer", code));
            return 1;
        }
        timer = Some(t);
    }

    let mut handles: Vec<&crate::Handle> = vec![&child.process];
    if let Some(t) = &timer {
        handles.push(t);
    }
    let wait_index = match sys.wait_for_multiple_objects(&handles, false) {
        Ok(i) => i,
        Err(code) => {
            // WAIT_FAILED: the child's actual state is unknown — best-effort
            // kill before giving up (caps.ps1 does the same).
            let mut msg = win32_err("WaitForMultipleObjects", code);
            if let Err(kill_code) = sys.terminate_process(&child.process, 1) {
                append_kill_failure(&mut msg, kill_code);
            }
            err(&msg);
            return 1;
        }
    };

    // When both handles are already signaled, WaitForMultipleObjects reports
    // the LOWEST index (the process), which proves nothing about ordering:
    // the kernel's own exit timestamp (GetProcessTimes, same FILETIME clock
    // as the timer's due time) is the tie-breaker. Strictly after the due
    // time -> treat exactly like the timer signaling.
    let mut timed_out = wait_index == 1;
    if wait_index == 0 && timer.is_some() {
        let exit_filetime = match sys.get_process_times(&child.process) {
            Ok(t) => t,
            Err(code) => {
                err(&win32_err("GetProcessTimes", code));
                return 1;
            }
        };
        if exit_filetime > due_time {
            timed_out = true;
        }
    }

    if timed_out {
        // Whole-tree kill in one atomic kernel call. 124 = unix timeout(1)
        // convention. NOT GetExitCodeProcess for the return value: the reason
        // for exiting is already known — but per the wnice contract we still
        // surface a GetExitCodeProcess failure as an error.
        if let Err(code) = sys.terminate_job_object(&job, 124) {
            err(&win32_err("TerminateJobObject", code));
            return 1;
        }
        if let Err(code) = sys.get_exit_code_process(&child.process) {
            err(&win32_err("GetExitCodeProcess", code));
            return 1;
        }
        err(&format!(
            "wnice: timed out after {} - job and every process in it were force-killed",
            timeout_display(cfg.timeout_ms.unwrap_or(0))
        ));
        return 124;
    }

    let exit_code = match sys.get_exit_code_process(&child.process) {
        Ok(c) => c,
        Err(code) => {
            err(&win32_err("GetExitCodeProcess", code));
            return 1;
        }
    };

    // Success path only: clear KILL_ON_JOB_CLOSE so the job handle's Drop
    // terminates nothing still assigned to the job (the documented
    // daemon-survival contract). Deliberately best-effort: a failed cleanup
    // syscall must not turn the child's real exit code into a wrapper error.
    // The CPU-rate cap is NOT released — the daemon keeping the cap is the
    // documented contract.
    let release_info = limits::release_extended_info(&applied);
    if let Err(code) = sys.set_information_job_object(
        &job,
        crate::JobObjectExtendedLimitInformation,
        &limits::extended_bytes(&release_info),
    ) {
        err(&format!(
            "warning: could not release the job's kill-on-close guard (SetInformationJobObject failed with Win32 error {code}) - a still-running background process left by the wrapped command may be terminated when this wrapper exits"
        ));
    }

    // Pass negative NTSTATUS codes through unchanged (bit-cast).
    exit_code as i32
}

#[cfg(test)]
pub(crate) mod fakesys;
#[cfg(test)]
mod tests;
