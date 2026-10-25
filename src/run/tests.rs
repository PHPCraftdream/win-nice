#![cfg(test)]
use super::fakesys::{Call, FakeSys};
use super::*;
use crate::PriorityClass;

static PROBE_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Create a unique temp dir containing the given (empty) files; returns
/// (dirs list, dir path).
fn probe_dir(files: &[&str]) -> (Vec<String>, std::path::PathBuf) {
    use std::sync::atomic::Ordering;
    let n = PROBE_SEQ.fetch_add(1, Ordering::SeqCst);
    let d = std::env::temp_dir().join(format!("wnice-run-test-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    for f in files {
        std::fs::write(d.join(f), b"").unwrap();
    }
    (vec![d.to_string_lossy().into_owned()], d)
}

fn collect_in(sys: &FakeSys, cfg: &Config, dirs: &[String]) -> (i32, Vec<String>) {
    let mut msgs: Vec<String> = Vec::new();
    let code = run_with_writer_in(cfg, sys, dirs, &mut |m| msgs.push(m.to_string()));
    (code, msgs)
}

/// Like `collect_in` but with a fresh probe dir holding an empty
/// `tool.exe`; returns (code, msgs, resolved abs path of tool.exe).
fn collect(sys: &FakeSys, cfg: &Config) -> (i32, Vec<String>, String) {
    let (dirs, dir) = probe_dir(&["tool.exe"]);
    let abs = dir.join("tool.exe").to_string_lossy().into_owned();
    let (code, msgs) = collect_in(sys, cfg, &dirs);
    let _ = std::fs::remove_dir_all(&dir);
    (code, msgs, abs)
}

fn basic_cfg() -> Config {
    Config {
        command: vec!["tool.exe".to_string(), "arg".to_string()],
        ..Default::default()
    }
}

const KILL: u32 = crate::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
const EXT: u32 = crate::JobObjectExtendedLimitInformation;

fn name(c: &Call) -> String {
    format!("{c:?}")
}

/// Reset the fake-handle drop counter; returns a guard reading it later —
/// use by resetting to 0 right before `collect`, then calling
/// [`assert_handles_balance`] after `collect` returns (all minted handles
/// have dropped by then).
fn reset_handle_drops() {
    crate::handle_test::FAKE_HANDLE_DROPS.with(|c| c.set(0));
}

/// Assert the drops counter equals the FakeSys's minted-handle count.
fn assert_handles_balance(sys: &FakeSys, ctx: &str) {
    let minted = sys.handles_minted.get();
    let drops = crate::handle_test::FAKE_HANDLE_DROPS.with(|c| c.get());
    assert_eq!(
        drops, minted as u32,
        "{ctx}: fake handles minted {minted} but only {drops} dropped"
    );
}

#[test]
fn drop_of_fake_handle_counts_once() {
    crate::handle_test::FAKE_HANDLE_DROPS.with(|c| c.set(0));
    let sys = FakeSys::new();
    let h = sys.handle();
    drop(h);
    crate::handle_test::FAKE_HANDLE_DROPS.with(|c| assert_eq!(c.get(), 1));
}

#[test]
fn happy_path_no_timeout_exact_sequence() {
    let sys = FakeSys::new().exit_code(3);
    let (code, msgs, tool_abs) = collect(&sys, &basic_cfg());
    assert_eq!(code, 3);
    assert!(msgs.is_empty());
    assert_eq!(
        sys.take(),
        vec![
            Call::SetJobInfo {
                class: EXT,
                flags: KILL
            },
            Call::CreateProcess {
                app: Some(tool_abs.clone()),
                cmdline: format!("{tool_abs} arg"),
                flags: crate::CREATE_SUSPENDED
            },
            Call::Assign,
            Call::Resume,
            Call::Wait,
            Call::GetExitCode,
            Call::SetJobInfo {
                class: EXT,
                flags: 0
            },
        ]
    );
}

#[test]
fn happy_all_limits_set_flags_and_release() {
    let sys = FakeSys::new().exit_code(0);
    let cfg = Config {
        threads: Some(4),
        memory: Some(crate::MemSpec::Bytes(1024)),
        procs: Some(2),
        cpu_percent: Some(30),
        command: vec!["tool.exe".to_string()],
        ..Default::default()
    };
    let (code, msgs, _) = collect(&sys, &cfg);
    assert_eq!(code, 0);
    assert!(msgs.is_empty());
    let setj = sys.set_job_calls();
    // [0] extended with all bits + KILL, [1] cpu class, [2] release minus KILL.
    assert_eq!(setj.len(), 3);
    let all = KILL
        | crate::JOB_OBJECT_LIMIT_AFFINITY
        | crate::JOB_OBJECT_LIMIT_JOB_MEMORY
        | crate::JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
    assert_eq!(setj[0], (EXT, all));
    assert_eq!(
        setj[1],
        (
            crate::JobObjectCpuRateControlInformation,
            0 // flags extraction only applies to the extended class
        )
    );
    assert_eq!(setj[2], (EXT, all & !KILL));
    assert_eq!(sys.count(|c| *c == Call::TerminateJobObject), 0);
    assert_eq!(sys.count(|c| *c == Call::TerminateProcess), 0);
    // No timeout configured: timer-related calls must not appear (the
    // plain process-handle Wait itself is expected in every run).
    assert_eq!(
        sys.count(|c| matches!(c, Call::CreateTimer | Call::SetTimer { .. })),
        0
    );
}

#[test]
fn priority_happy_two_applies() {
    let sys = FakeSys::new().exit_code(0);
    let cfg = Config {
        priority: Some(PriorityClass::AboveNormal),
        command: vec!["tool.exe".to_string()],
        ..Default::default()
    };
    let (code, msgs, _) = collect(&sys, &cfg);
    assert_eq!(code, 0);
    assert!(msgs.is_empty());
    let calls = sys.take();
    let setj: Vec<(u32, u32)> = calls
        .iter()
        .filter_map(|c| match c {
            Call::SetJobInfo { class, flags } => Some((*class, *flags)),
            _ => None,
        })
        .collect();
    // Two-step design: apply without prio, probe with prio, release
    // (release keeps the WITH-priority limits minus KILL since the probe
    // succeeded and `applied` was upgraded).
    assert_eq!(setj.len(), 3);
    assert_eq!(setj[0], (EXT, KILL));
    assert_eq!(
        setj[1],
        (EXT, KILL | crate::JOB_OBJECT_LIMIT_PRIORITY_CLASS)
    );
    assert_eq!(setj[2], (EXT, crate::JOB_OBJECT_LIMIT_PRIORITY_CLASS));
    // creation flags include the class value.
    assert!(calls.iter().any(|c| matches!(
        c,
        Call::CreateProcess { flags, .. }
            if *flags == crate::CREATE_SUSPENDED | PriorityClass::AboveNormal.to_u32()
    )));
}

#[test]
fn priority_1314_note_and_creation_flags() {
    let sys = FakeSys::new()
        // skip the no-priority baseline apply; hit the with-priority probe
        .fail_nth("SetInformationJobObject", 1, 1314)
        .exit_code(0);
    let cfg = Config {
        priority: Some(PriorityClass::AboveNormal),
        command: vec!["tool.exe".to_string()],
        ..Default::default()
    };
    let (code, msgs, _) = collect(&sys, &cfg);
    assert_eq!(code, 0);
    assert_eq!(
            msgs,
            vec![
                "wnice: note: abovenormal priority applies to the root process only (the job refused tree-wide priority: Win32 error 1314)".to_string()
            ]
        );
    let calls = sys.take();
    let setj: Vec<(u32, u32)> = calls
        .iter()
        .filter_map(|c| match c {
            Call::SetJobInfo { class, flags } => Some((*class, *flags)),
            _ => None,
        })
        .collect();
    // Two logged applies (baseline + release); the failed probe leaves no
    // log entry. The release re-asserts the no-priority baseline with the
    // kill-on-close guard cleared (all other flags kept).
    assert_eq!(setj.len(), 2);
    assert_eq!(setj[0], (EXT, KILL)); // without prio
    assert_eq!(setj[1], (EXT, 0)); // release of the no-prio baseline
    assert!(calls.iter().any(|c| matches!(
        c,
        Call::CreateProcess { flags, .. }
            if *flags == crate::CREATE_SUSPENDED | PriorityClass::AboveNormal.to_u32()
    )));
    assert_handles_balance(&sys, "priority_1314_note_and_creation_flags");
}

#[test]
fn priority_realtime_1314_becomes_high_0x80() {
    let sys = FakeSys::new()
        .fail_nth("SetInformationJobObject", 1, 1314)
        .exit_code(0);
    let cfg = Config {
        priority: Some(PriorityClass::Realtime),
        command: vec!["tool.exe".to_string()],
        ..Default::default()
    };
    let (code, _, _) = collect(&sys, &cfg);
    assert_eq!(code, 0);
    let calls = sys.take();
    assert!(calls.iter().any(|c| matches!(
        c,
        Call::CreateProcess { flags, .. } if *flags == crate::CREATE_SUSPENDED | 0x80
    )));
    assert_handles_balance(&sys, "priority_realtime_1314_becomes_high_0x80");
}

#[test]
fn priority_other_error_is_fatal() {
    let sys = FakeSys::new().fail_nth("SetInformationJobObject", 1, 5); // the with-priority probe
    let cfg = Config {
        priority: Some(PriorityClass::High),
        command: vec!["tool.exe".to_string()],
        ..Default::default()
    };
    reset_handle_drops();
    let (code, msgs, _) = collect(&sys, &cfg);
    assert_eq!(code, 1);
    assert_eq!(msgs, vec!["SetInformationJobObject failed: 5"]);
    // Failure is pre-CreateProcess: no child, no kills.
    let calls = sys.take();
    assert!(!calls.iter().any(|c| matches!(
        c,
        Call::CreateProcess { .. } | Call::TerminateProcess | Call::TerminateJobObject
    )));
    assert_handles_balance(&sys, "priority_other_error_is_fatal");
}

#[test]
fn each_fallible_call_failing_one_at_a_time() {
    // (fn, injected code, expected message, expect TerminateProcess attempted)
    let cases: &[(&'static str, u32, &str, bool)] = &[
        ("CreateJobObject", 8, "CreateJobObject failed: 8", false),
        (
            "SetInformationJobObject",
            87,
            "SetInformationJobObject failed: 87",
            false,
        ),
        ("CreateProcess", 2, "CreateProcess failed: 2", false),
        (
            "AssignProcessToJobObject",
            1,
            "AssignProcessToJobObject failed: 1",
            true,
        ),
        ("ResumeThread", 6, "ResumeThread failed: 6", true),
        (
            "CreateWaitableTimer",
            5,
            "CreateWaitableTimer failed: 5",
            false,
        ),
        ("SetWaitableTimer", 87, "SetWaitableTimer failed: 87", false),
        (
            "WaitForMultipleObjects",
            6,
            "WaitForMultipleObjects failed: 6",
            true,
        ),
        ("GetProcessTimes", 87, "GetProcessTimes failed: 87", false),
        (
            "TerminateJobObject",
            5,
            "TerminateJobObject failed: 5",
            false,
        ),
        (
            "GetExitCodeProcess",
            6,
            "GetExitCodeProcess failed: 6",
            false,
        ),
    ];
    for (fname, code, expected, expect_kill) in cases {
        let mut sys = FakeSys::new().fail_next(fname, *code);
        if *fname == "CreateProcess" {
            // The first failure hits the direct attempt; the second one
            // must hit the cmd.exe fallback so the error surfaces.
            sys = sys.fail_next(fname, *code);
        }
        // These paths only run when a timeout timer exists.
        let needs_timer = matches!(
            *fname,
            "TerminateJobObject"
                | "GetProcessTimes"
                | "GetExitCodeProcess"
                | "CreateWaitableTimer"
                | "SetWaitableTimer"
        );
        if needs_timer {
            // GetProcessTimes is only called on the tie-break (index 0);
            // the others exercise the timeout path (index 1).
            sys = sys.wait_on(if *fname == "GetProcessTimes" { 0 } else { 1 });
        }
        let cfg = if needs_timer {
            Config {
                timeout_ms: Some(500),
                ..basic_cfg()
            }
        } else {
            basic_cfg()
        };
        reset_handle_drops();
        let (exit, msgs, _) = collect(&sys, &cfg);
        assert_eq!(exit, 1, "{fname}");
        assert_eq!(msgs, vec![expected.to_string()], "{fname}");
        assert_eq!(
            sys.count(|c| *c == Call::TerminateProcess),
            usize::from(*expect_kill),
            "{fname}"
        );
        assert_handles_balance(&sys, fname);
    }
}

#[test]
fn create_waitable_timer_failure_kills_only_via_job_drop() {
    let sys = FakeSys::new().fail_next("CreateWaitableTimer", 5);
    let cfg = Config {
        timeout_ms: Some(1000),
        ..basic_cfg()
    };
    let (code, msgs, _) = collect(&sys, &cfg);
    assert_eq!(code, 1);
    assert_eq!(msgs, vec!["CreateWaitableTimer failed: 5"]);
    let calls = sys.take();
    assert!(!calls.iter().any(|c| matches!(
        c,
        Call::TerminateProcess | Call::TerminateJobObject | Call::SetTimer { .. }
    )));
    assert_handles_balance(
        &sys,
        "create_waitable_timer_failure_kills_only_via_job_drop",
    );
}

#[test]
fn assign_and_resume_double_failure_suffix() {
    for fname in ["AssignProcessToJobObject", "ResumeThread"] {
        let sys = FakeSys::new()
            .fail_next(fname, 5)
            .fail_next("TerminateProcess", 6);
        reset_handle_drops();
        let (code, msgs, _) = collect(&sys, &basic_cfg());
        assert_eq!(code, 1);
        assert_eq!(
            msgs,
            vec![format!(
                "{fname} failed: 5; TerminateProcess also failed: 6"
            )]
        );
        assert_handles_balance(&sys, fname);
    }
}

#[test]
fn wait_failure_double_failure_suffix() {
    let sys = FakeSys::new()
        .fail_next("WaitForMultipleObjects", 6)
        .fail_next("TerminateProcess", 87);
    reset_handle_drops();
    let (code, msgs, _) = collect(&sys, &basic_cfg());
    assert_eq!(code, 1);
    assert_eq!(
        msgs,
        vec!["WaitForMultipleObjects failed: 6; TerminateProcess also failed: 87"]
    );
    assert_handles_balance(&sys, "WaitForMultipleObjects");
}

#[test]
fn direct_create_process_fails_falls_back_to_shell() {
    let sys = FakeSys::new().fail_next("CreateProcess", 2).exit_code(0);
    let (code, msgs, _) = collect(&sys, &basic_cfg());
    assert_eq!(code, 0);
    assert!(msgs.is_empty());
    let calls = sys.take();
    let launches: Vec<&Call> = calls
        .iter()
        .filter(|c| matches!(c, Call::CreateProcess { .. }))
        .collect();
    assert_eq!(launches.len(), 1);
    assert!(matches!(
        launches[0],
        Call::CreateProcess { app: None, ref cmdline, .. }
            if cmdline.starts_with("\"C:\\Windows\\System32\\cmd.exe\" /d /v:off /s /c ")
                && cmdline.contains("tool.exe")
    ));
    assert_handles_balance(&sys, "direct_create_process_fails_falls_back_to_shell");
}

#[test]
fn bare_name_not_in_dirs_exit_127_command_not_found() {
    let sys = FakeSys::new();
    let (dirs, dir) = probe_dir(&[]);
    let cfg = Config {
        command: vec!["tool.exe".to_string()],
        ..Default::default()
    };
    let (code, msgs) = collect_in(&sys, &cfg, &dirs);
    assert_eq!(code, 127);
    assert!(
        msgs.len() == 1 && msgs[0].contains("command not found"),
        "{msgs:?}"
    );
    // Nothing was launched.
    assert_eq!(sys.count(|c| matches!(c, Call::CreateProcess { .. })), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bat_resolution_launches_shell_with_abs_path() {
    let sys = FakeSys::new().exit_code(0);
    let (dirs, dir) = probe_dir(&["foo.bat"]);
    let cfg = Config {
        command: vec!["foo.bat".to_string(), "arg".to_string()],
        ..Default::default()
    };
    let (code, msgs) = collect_in(&sys, &cfg, &dirs);
    assert_eq!(code, 0);
    assert!(msgs.is_empty());
    let bat_abs = dir.join("foo.bat").to_string_lossy().into_owned();
    assert!(sys.take().iter().any(|c| matches!(
        c,
        Call::CreateProcess { app: None, cmdline, .. }
            if cmdline.starts_with("\"C:\\Windows\\System32\\cmd.exe\" /d /v:off /s /c ")
                && cmdline.contains(&bat_abs)
    )));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn percent_refusal_on_fallback_before_launch() {
    let sys = FakeSys::new().fail_next("CreateProcess", 2);
    let cfg = Config {
        command: vec!["tool.exe".to_string(), "100%OFF".to_string()],
        ..Default::default()
    };
    let (code, msgs, _) = collect(&sys, &cfg);
    assert_eq!(code, 1);
    assert_eq!(msgs.len(), 1);
    assert!(msgs[0].starts_with("wnice: Refusing to run: argument contains '%'"));
    assert_eq!(sys.count(|c| matches!(c, Call::CreateProcess { .. })), 0); // only the failed direct attempt, unlogged
    assert_handles_balance(&sys, "percent_refusal_on_fallback_before_launch");
}

#[test]
fn bat_with_percent_refused_before_any_create_process() {
    let sys = FakeSys::new();
    let (dirs, dir) = probe_dir(&["foo.bat"]);
    let cfg = Config {
        command: vec!["foo.bat".to_string(), "100%".to_string()],
        ..Default::default()
    };
    let (code, msgs) = collect_in(&sys, &cfg, &dirs);
    assert_eq!(code, 1);
    assert_eq!(msgs.len(), 1);
    assert!(msgs[0].starts_with("wnice: Refusing to run: argument contains '%'"));
    assert_eq!(
        sys.count(|c| matches!(c, Call::CreateProcess { .. })),
        0,
        "refusal must happen before any CreateProcess on the bat/cmd path"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn timeout_path_terminates_job_exit_124() {
    let sys = FakeSys::new().wait_on(1).exit_code(0);
    let cfg = Config {
        timeout_ms: Some(2000),
        ..basic_cfg()
    };
    let (code, msgs, _) = collect(&sys, &cfg);
    assert_eq!(code, 124);
    assert_eq!(
        msgs,
        vec!["wnice: timed out after 2s - job and every process in it were force-killed"]
    );
    let calls = sys.take();
    assert_eq!(
        calls
            .iter()
            .filter(|c| **c == Call::TerminateJobObject)
            .count(),
        1
    );
    assert!(calls.contains(&Call::CreateTimer));
    assert!(
        matches!(calls.iter().find(|c| matches!(c, Call::SetTimer { .. })), Some(Call::SetTimer { due }) if *due > 0)
    );
    // Release never runs on the timeout path.
    let setj = calls
        .iter()
        .filter(|c| matches!(c, Call::SetJobInfo { .. }))
        .count();
    assert_eq!(setj, 1); // only the initial extended apply
}

#[test]
fn timeout_display_formats() {
    assert_eq!(timeout_display(2000), "2s");
    assert_eq!(timeout_display(2500), "2.5s");
    assert_eq!(timeout_display(1234), "1.234s");
    assert_eq!(timeout_display(1000), "1s");
    assert_eq!(timeout_display(100), "0.1s");
}

#[test]
fn tie_break_exit_after_due_is_timeout() {
    let sys = FakeSys::new().wait_on(0).exit_time_delta(1); // exited strictly after the deadline
    let cfg = Config {
        timeout_ms: Some(500),
        ..basic_cfg()
    };
    let (code, msgs, _) = collect(&sys, &cfg);
    assert_eq!(code, 124);
    assert!(msgs[0].starts_with("wnice: timed out after 0.5s"));
    assert_eq!(sys.count(|c| *c == Call::TerminateJobObject), 1);
    assert_eq!(sys.count(|c| *c == Call::GetProcessTimes), 1);
}

#[test]
fn tie_break_exit_at_or_before_due_is_normal() {
    let sys = FakeSys::new().wait_on(0).exit_code(9).exit_time_delta(0); // exactly at the deadline counts as on-time
    let cfg = Config {
        timeout_ms: Some(500),
        ..basic_cfg()
    };
    let (code, msgs, _) = collect(&sys, &cfg);
    assert_eq!(code, 9);
    assert!(msgs.is_empty());
    assert_eq!(sys.count(|c| *c == Call::TerminateJobObject), 0);
    assert_eq!(sys.count(|c| *c == Call::GetProcessTimes), 1);
}

#[test]
fn get_process_times_failure_on_tie_break() {
    let sys = FakeSys::new().wait_on(0).fail_next("GetProcessTimes", 87);
    let cfg = Config {
        timeout_ms: Some(500),
        ..basic_cfg()
    };
    let (code, msgs, _) = collect(&sys, &cfg);
    assert_eq!(code, 1);
    assert_eq!(msgs, vec!["GetProcessTimes failed: 87"]);
    assert_handles_balance(&sys, "get_process_times_failure_on_tie_break");
}

#[test]
fn get_exit_code_failure_on_timeout_path() {
    let sys = FakeSys::new().wait_on(1).fail_next("GetExitCodeProcess", 6);
    let cfg = Config {
        timeout_ms: Some(500),
        ..basic_cfg()
    };
    let (code, msgs, _) = collect(&sys, &cfg);
    assert_eq!(code, 1);
    assert_eq!(msgs, vec!["GetExitCodeProcess failed: 6"]);
    assert_eq!(sys.count(|c| *c == Call::TerminateJobObject), 1);
    assert_handles_balance(&sys, "get_exit_code_failure_on_timeout_path");
}

#[test]
fn release_failure_warns_keeps_exit_code() {
    // No priority -> only one initial SetJobInfo call, so the single
    // queued failure lands on the release call.
    let sys = FakeSys::new()
        .exit_code(7)
        .fail_nth("SetInformationJobObject", 1, 5); // skip baseline, hit release
    let cfg = basic_cfg();
    let (code, msgs, _) = collect(&sys, &cfg);
    assert_eq!(code, 7);
    assert_eq!(
            msgs,
            vec!["warning: could not release the job's kill-on-close guard (SetInformationJobObject failed with Win32 error 5) - a still-running background process left by the wrapped command may be terminated when this wrapper exits"]
        );
    assert_handles_balance(&sys, "release_failure_warns_keeps_exit_code");
}

#[test]
fn empty_config_kill_only_then_zero_release() {
    let sys = FakeSys::new().exit_code(0);
    let cfg = Config {
        command: vec!["tool.exe".to_string()],
        ..Default::default()
    };
    let (code, _, _) = collect(&sys, &cfg);
    assert_eq!(code, 0);
    let setj = sys.set_job_calls();
    assert_eq!(setj, vec![(EXT, KILL), (EXT, 0)]);
}

#[test]
fn compose_error_reported_exit_1() {
    let sys = FakeSys::new();
    let cfg = Config {
        threads: Some(99),
        command: vec!["tool.exe".to_string()],
        ..Default::default()
    };
    let (code, msgs, _) = collect(&sys, &cfg);
    assert_eq!(code, 1);
    assert_eq!(msgs.len(), 1);
    assert!(msgs[0].starts_with("wnice: -t/--threads is out of range"));
    assert!(sys.take().is_empty(), "no job is created on compose error");
}

#[test]
fn launch_plan_error_reported_exit_1() {
    let sys = FakeSys::new();
    let (code, msgs, _) = collect(&sys, &Config::default());
    assert_eq!(code, 1);
    assert_eq!(msgs, vec!["no command given"]);
    // The job exists by then, but nothing beyond creation happened.
    let calls = sys.take();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0],
        Call::SetJobInfo {
            class: EXT,
            flags: KILL
        }
    );
}

#[test]
fn set_information_job_object_limits_failure_before_priority_launch() {
    // The FIRST apply (no-priority baseline) failing is fatal before any
    // child exists.
    let sys = FakeSys::new().fail_next("SetInformationJobObject", 87);
    let cfg = Config {
        priority: Some(PriorityClass::High),
        ..basic_cfg()
    };
    let (code, msgs, _) = collect(&sys, &cfg);
    assert_eq!(code, 1);
    assert_eq!(msgs, vec!["SetInformationJobObject failed: 87"]);
    assert_eq!(sys.count(|c| matches!(c, Call::CreateProcess { .. })), 0);
    assert_handles_balance(
        &sys,
        "set_information_job_object_limits_failure_before_priority_launch",
    );
}

#[test]
fn cpu_rate_failure_is_fatal() {
    // Target the CPU-rate apply (info class 15) specifically, NOT the
    // extended-limits apply.
    let sys = FakeSys::new().fail_next("SetInformationJobObject:15", 87);
    let cfg = Config {
        cpu_percent: Some(50),
        ..basic_cfg()
    };
    reset_handle_drops();
    let (code, msgs, _) = collect(&sys, &cfg);
    assert_eq!(code, 1);
    assert_eq!(msgs, vec!["SetInformationJobObject failed: 87"]);
    assert_eq!(sys.count(|c| matches!(c, Call::CreateProcess { .. })), 0);
    // The CPU-rate apply failed, so no class-15 SetJobInfo was logged.
    assert_eq!(
        sys.count(|c| matches!(
            c,
            Call::SetJobInfo {
                class: crate::JobObjectCpuRateControlInformation,
                ..
            }
        )),
        0
    );
    assert_handles_balance(&sys, "cpu_rate_failure_is_fatal");
}

/// Self-check for `cpu_rate_failure_is_fatal`: WITHOUT any injected failure
/// the same config succeeds and the class-15 apply happens. If someone
/// makes the CPU-rate call infallible (or skips it), the failure above
/// would never fire and this would be the only signal.
#[test]
fn cpu_rate_success_applies_class_15() {
    let sys = FakeSys::new().exit_code(0);
    let cfg = Config {
        cpu_percent: Some(50),
        ..basic_cfg()
    };
    let (code, msgs, _) = collect(&sys, &cfg);
    assert_eq!(code, 0);
    assert!(msgs.is_empty());
    let setj = sys.set_job_calls();
    assert_eq!(
        setj,
        vec![
            (EXT, KILL),
            (crate::JobObjectCpuRateControlInformation, 0),
            (EXT, 0),
        ]
    );
}

#[test]
fn negative_ntstatus_passes_through() {
    let sys = FakeSys::new().exit_code(0xC000_013A);
    let (code, _, _) = collect(&sys, &basic_cfg());
    assert_eq!(code, -1073741510i32);
}

#[test]
fn call_order_smoke() {
    let sys = FakeSys::new().exit_code(1);
    let cfg = Config {
        timeout_ms: Some(1000),
        ..basic_cfg()
    };
    let _ = collect(&sys, &cfg);
    let names: Vec<String> = sys.take().iter().map(name).collect();
    let assign = names.iter().position(|n| n == "Assign").unwrap();
    let resume = names.iter().position(|n| n == "Resume").unwrap();
    let timer = names.iter().position(|n| n == "CreateTimer").unwrap();
    let wait = names.iter().position(|n| n == "Wait").unwrap();
    let getexit = names.iter().position(|n| n == "GetExitCode").unwrap();
    assert!(assign < resume);
    assert!(resume < timer);
    assert!(timer < wait);
    assert!(wait < getexit);
}
