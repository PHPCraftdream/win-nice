//! Integration tests (harness = false custom main): run the real `wnice` exe
//! (CARGO_BIN_EXE_wnice) against probe children. The probe is THIS test
//! binary re-executing itself; `main()` dispatches to `probe_main` when
//! `WNICE_PROBE` is set (inherited through wnice, which spawns the probe with
//! a NULL environment = inherit).
//!
//! WHY harness = false: with libtest, a probe process re-ran ALL test fns in
//! parallel; each fn's probe_entry() guard only stopped the FIRST fn, so the
//! remaining fns ran their real bodies, spawned more wnice/probe processes,
//! and multiplied recursively (process flood). Now a probe process runs ONLY
//! the probe and exits — no test body can execute in it.
//!
//! Arg passing contract: `wnice <options> -- <probe_exe> <mode> [extra...]`;
//! the mode is BOTH the first post-`--` argument and the `WNICE_PROBE` env var
//! (belt and suspenders; dispatch reads the env var, extras come from argv).
//!
//! Windows-only; light (no synthetic CPU load, no realtime-priority tests,
//! kills only by PID of processes this test started).

#![cfg(windows)]

mod probe_support;
use probe_support::*;

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use windows_sys::Win32::System::JobObjects::{
    JOB_OBJECT_CPU_RATE_CONTROL_ENABLE, JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

// ------------------------------------------------------------- test helpers

/// Run the built wnice exe with `wnice_args`, then `--`, the probe exe, the
/// probe mode, and extra probe args. Sets WNICE_PROBE so the probe dispatches.
/// Depth-guarded: wnice re-spawns the probe (env = inherit, so WNICE_PROBE_DEPTH
/// propagates through wnice to the probe automatically).
fn wnice_probe(wnice_args: &[&str], mode: &str, probe_args: &[&str]) -> Output {
    check_probe_depth();
    Command::new(env!("CARGO_BIN_EXE_wnice"))
        .creation_flags(CREATE_NO_WINDOW)
        .args(wnice_args)
        .arg("--")
        .arg(probe_exe())
        .args(probe_args) // REAL argv after `--` (echo mode reads it)
        .env("WNICE_PROBE", mode)
        .env("WNICE_PROBE_ARGS", probe_args.join("\u{1f}")) // belt-and-suspenders for env-reading modes
        .env("WNICE_PROBE_DEPTH", (probe_depth() + 1).to_string())
        .output()
        .expect("spawn wnice")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// Last non-empty stdout line: wnice may print notes before the probe's own
/// output, so consumers parse the tail.
fn last_line(o: &Output) -> String {
    stdout(o)
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn unique_temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("wnice-itest-{}-{}", std::process::id(), name));
    std::fs::create_dir_all(&d).expect("temp dir");
    d
}

// ----------------------------------------------------------- skip support --

thread_local! {
    static SKIP_REASON: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

/// Signal a skip from inside a guarded test: set the reason and return. The
/// main loop picks it up after catch_unwind (skip paths never panic).
fn skip(reason: &str) {
    SKIP_REASON.with(|r| *r.borrow_mut() = Some(reason.to_string()));
}

// ------------------------------------------------------------------ tests --

fn test_exit_code_propagation() {
    let o = wnice_probe(&["-s", "10"], "exit", &["5"]);
    assert_eq!(o.status.code(), Some(5), "stdout={}", stdout(&o));

    let o = wnice_probe(&["-s", "10"], "sleep", &["200"]);
    assert_eq!(o.status.code(), Some(0));
}

fn test_negative_exit_code() {
    // probe exits via process::exit(-1 as i32); Windows exit codes are full
    // 32-bit, so the parent must observe 0xFFFFFFFF (Some(-1)).
    let o = wnice_probe(&["-s", "10"], "exit", &["-1"]);
    assert_eq!(o.status.code(), Some(-1), "raw={:?}", o.status.code());
}

fn test_timeout_kills_whole_tree() {
    let start = Instant::now();
    let o = wnice_probe(&["-s", "2"], "spawnchain", &["15000"]);
    let elapsed = start.elapsed();
    assert_eq!(o.status.code(), Some(124), "stderr={}", stderr(&o));
    assert!(elapsed < Duration::from_secs(10), "elapsed={elapsed:?}");

    let fields: Vec<u32> = last_line(&o)
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect();
    assert!(fields.len() >= 6, "spawnchain output: {}", stdout(&o));
    // Give the kernel a moment to finish termination bookkeeping.
    std::thread::sleep(Duration::from_millis(300));
    assert!(!pid_alive(fields[0]), "root survived timeout");
    assert!(!pid_alive(fields[1]), "child survived timeout");
    assert!(!pid_alive(fields[2]), "grandchild survived timeout");
}

fn test_priority_job_wide() {
    if guard_job_forces_idle() {
        skip("sandbox job forces IDLE priority class; wnice cannot raise tree priority above the cap");
        return;
    }
    for (flag, name, class) in [
        ("-p", "abovenormal", 0x8000u32),
        ("-p", "idle", 0x40u32),
        ("-p", "belownormal", 0x4000u32),
    ] {
        let o = wnice_probe(&[flag, name], "priority", &[]);
        assert_eq!(o.status.code(), Some(0), "{flag} {name}: {}", stderr(&o));
        let got: u32 = last_line(&o).parse().expect("priority");
        assert_eq!(got, class, "{flag} {name}");
    }
    // Grandchild inheritance: whole tree at idle.
    let o = wnice_probe(&["-p", "idle"], "spawnchain", &["4000"]);
    let fields: Vec<u32> = last_line(&o)
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect();
    assert!(fields.len() >= 6, "spawnchain output: {}", stdout(&o));
    assert_eq!(fields[3], 0x40, "self prio");
    assert_eq!(fields[4], 0x40, "child prio");
    assert_eq!(fields[5], 0x40, "grandchild prio");
    for pid in &fields[..3] {
        kill_pid(*pid);
    }
}

fn test_priority_fallback_note_or_elevated() {
    if guard_job_forces_idle() {
        skip("sandbox job forces IDLE priority class; neither the note nor the elevated outcome is observable");
        return;
    }
    let o = wnice_probe(&["-p", "high"], "priority", &[]);
    assert_eq!(o.status.code(), Some(0), "stderr={}", stderr(&o));
    let got: u32 = last_line(&o).parse().expect("priority");
    if stderr(&o).contains("wnice: note: high priority applies to the root process only") {
        // Job refused tree-wide priority: wnice ORs HIGH_PRIORITY_CLASS into
        // the root's creation flags, so the root is High even unprivileged.
        assert_eq!(got, 0x80, "expected High (0x80) for the root");
    } else {
        // Elevated: job accepted priority, root is High.
        assert_eq!(got, 0x80, "expected High when elevated");
    }
}

fn test_affinity_single_cpu() {
    let o = wnice_probe(&["-t", "1"], "affinity", &[]);
    assert_eq!(o.status.code(), Some(0), "stderr={}", stderr(&o));
    let mask: u64 = last_line(&o).parse().expect("mask");
    assert_eq!(mask, 1, "single-CPU affinity mask");
}

fn test_memory_limit_enforced() {
    // 200 MB against a 100 MB job limit: allocation must not succeed.
    let o = wnice_probe(&["-m", "100m"], "allocmem", &["200"]);
    assert_ne!(o.status.code(), Some(0), "over-limit alloc succeeded?!");
    assert!(!stdout(&o).contains("ok"));

    // 50 MB fits comfortably.
    let o = wnice_probe(&["-m", "100m"], "allocmem", &["50"]);
    assert_eq!(o.status.code(), Some(0), "stderr={}", stderr(&o));
    assert!(stdout(&o).contains("ok"));
}

fn test_procs_limit() {
    // Root counts itself: with -n 1 the probe's own spawn must be refused.
    let o = wnice_probe(&["-n", "1"], "spawnfail", &[]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "root must stay alive: {}",
        stderr(&o)
    );
    assert!(stdout(&o).contains("spawn-failed"), "stdout={}", stdout(&o));

    let o = wnice_probe(&["-n", "3"], "spawnfail", &[]);
    assert_eq!(o.status.code(), Some(0));
    assert!(stdout(&o).contains("spawned"), "stdout={}", stdout(&o));
    if let Some(pid) = last_line(&o)
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
    {
        kill_pid(pid);
    }
}

fn test_daemon_survives_wrapper_exit() {
    // Real-world daemon pattern: the wrapped command starts a background
    // process with its own hidden console (Start-Process) and exits at once.
    // A Rust Command-spawned DETACHED sleeper is no oracle here: it also died
    // when its parent exited with a piped stdout, with no wnice in the chain.
    let script = format!(
        "(Start-Process -FilePath '{}' -WindowStyle Hidden -PassThru).Id",
        probe_exe().display()
    );
    check_probe_depth();
    let o = Command::new(env!("CARGO_BIN_EXE_wnice"))
        .creation_flags(CREATE_NO_WINDOW)
        .args([
            "-c",
            "50",
            "--",
            "powershell",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
        ])
        .arg(&script)
        .env("WNICE_PROBE", "sleep")
        .env("WNICE_PROBE_ARGS", "30000")
        .env("WNICE_PROBE_DEPTH", (probe_depth() + 1).to_string())
        .output()
        .expect("spawn wnice");
    assert_eq!(o.status.code(), Some(0), "stderr={}", stderr(&o));
    let pid: u32 = last_line(&o).trim().parse().expect("child pid");
    // The detached child must stay alive through the whole budget (max 5 s):
    // KILL_ON_JOB_CLOSE was released, so it outlives the wnice wrapper.
    let mut survived = true;
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(250));
        if !pid_alive(pid) {
            survived = false;
            break;
        }
    }
    assert!(survived, "daemon child died with the wrapper");
    assert!(kill_pid(pid), "cleanup kill failed for pid {pid}");
    // Terminated by explicit kill: must be gone shortly after.
    let mut dead = false;
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(50));
        if !pid_alive(pid) {
            dead = true;
            break;
        }
    }
    assert!(dead, "child survived explicit kill");
}

fn test_no_shell_layer_between() {
    let out = Command::new(env!("CARGO_BIN_EXE_wnice"))
        .creation_flags(CREATE_NO_WINDOW)
        .args(["--"])
        .arg(probe_exe())
        .env("WNICE_PROBE", "parentpid")
        .env("WNICE_PROBE_DEPTH", (probe_depth() + 1).to_string())
        .output()
        .expect("spawn wnice");
    assert_eq!(out.status.code(), Some(0), "stderr={}", stderr(&out));
    let _parent: u32 = stdout(&out).trim().parse().expect("parent pid");
    // Re-derive wnice's pid precisely: spawn once holding the Child handle.
    let child = Command::new(env!("CARGO_BIN_EXE_wnice"))
        .creation_flags(CREATE_NO_WINDOW)
        .args(["--"])
        .arg(probe_exe())
        .env("WNICE_PROBE", "parentpid")
        .env("WNICE_PROBE_DEPTH", (probe_depth() + 1).to_string())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn wnice 2");
    let wnice_pid = child.id();
    let o = child.wait_with_output().expect("wait");
    let probe_parent: u32 = last_line(&o).parse().expect("parent pid 2");
    assert_eq!(probe_parent, wnice_pid);
    assert_ne!(
        probe_parent,
        std::process::id(),
        "probe parent must be wnice, not the test"
    );
}

fn test_bat_target_with_space_in_path() {
    let dir = unique_temp("bat");
    // Path WITH a space to exercise quoting through the cmd.exe fallback.
    let bat = dir.join("wnice itest.bat");
    std::fs::write(&bat, "@echo BATOK\r\nexit /b 0\r\n").expect("write bat");
    let o = Command::new(env!("CARGO_BIN_EXE_wnice"))
        .creation_flags(CREATE_NO_WINDOW)
        .arg("--")
        .arg(&bat)
        .output()
        .expect("spawn wnice");
    assert_eq!(o.status.code(), Some(0), "stderr={}", stderr(&o));
    assert!(stdout(&o).contains("BATOK"), "stdout={}", stdout(&o));
    let _ = std::fs::remove_dir_all(&dir);
}

fn test_bat_percent_argument_refused() {
    let dir = unique_temp("batpct");
    let bat = dir.join("marker.bat");
    let marker = dir.join("marker.txt");
    std::fs::write(
        &bat,
        format!("@echo ran > \"{}\"\r\nexit /b 0\r\n", marker.display()),
    )
    .expect("write bat");
    let o = Command::new(env!("CARGO_BIN_EXE_wnice"))
        .creation_flags(CREATE_NO_WINDOW)
        .arg("--")
        .arg(&bat)
        .arg("100%")
        .output()
        .expect("spawn wnice");
    assert_eq!(o.status.code(), Some(1), "stdout={}", stdout(&o));
    assert!(stderr(&o).contains("wnice:"), "stderr={}", stderr(&o));
    assert!(!Path::new(&marker).exists(), "bat must not have run");
    let _ = std::fs::remove_dir_all(&dir);
}

fn test_cpu_rate_cap_visible_to_probe() {
    let o = wnice_probe(&["-c", "50"], "jobinfo", &[]);
    assert_eq!(o.status.code(), Some(0), "stderr={}", stderr(&o));
    // "limitflags=<dec> cpuctrl=<dec> cpurate=<dec>"
    let vals: Vec<u32> = last_line(&o)
        .split_whitespace()
        .filter_map(|t| t.split('=').nth(1).and_then(|v| v.parse().ok()))
        .collect();
    assert!(vals.len() >= 3, "jobinfo output: {}", stdout(&o));
    assert_ne!(
        vals[0] & JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        0,
        "KILL_ON_JOB_CLOSE must be set while running"
    );
    assert_eq!(
        vals[1],
        JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
        "cpu rate control flags"
    );
    assert_eq!(vals[2], 5000, "CpuRate for 50% must be 5000");
}

fn test_version_and_help() {
    let o = Command::new(env!("CARGO_BIN_EXE_wnice"))
        .creation_flags(CREATE_NO_WINDOW)
        .arg("-V")
        .output()
        .expect("spawn wnice");
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(
        stdout(&o).trim(),
        format!("wnice {}", env!("CARGO_PKG_VERSION"))
    );

    let o = Command::new(env!("CARGO_BIN_EXE_wnice"))
        .creation_flags(CREATE_NO_WINDOW)
        .arg("-h")
        .output()
        .expect("spawn wnice");
    assert_eq!(o.status.code(), Some(0));
    assert!(stdout(&o).contains("usage:"));
}

fn test_post_double_dash_tokens_verbatim() {
    // Options still parsed before `--`; everything after goes verbatim —
    // including things that LOOK like wnice flags and a literal `%`.
    let o = wnice_probe(&["-c", "5"], "exit", &["7"]);
    assert_eq!(o.status.code(), Some(7));

    let o = wnice_probe(
        &["-c", "5"],
        "echo",
        &[
            "a", "-p", "idle", "100%", "x y", "q\"q", "A&B", "back\\", "%PATH%",
        ],
    );
    assert_eq!(o.status.code(), Some(0), "stderr={}", stderr(&o));
    let expected = [
        "a", "-p", "idle", "100%", "x y", "q\"q", "A&B", "back\\", "%PATH%",
    ]
    .join("\u{1f}");
    assert_eq!(last_line(&o), expected);
}

fn test_depth_guard_refuses_at_max() {
    // A probe at WNICE_PROBE_DEPTH = MAX_PROBE_DEPTH must refuse to spawn;
    // the "guardedspawn" mode catches the guard panic and reports.
    let o = Command::new(probe_exe())
        .creation_flags(CREATE_NO_WINDOW)
        .env("WNICE_PROBE", "guardedspawn")
        .env("WNICE_PROBE_DEPTH", "3")
        .output()
        .expect("spawn probe at max depth");
    assert_eq!(o.status.code(), Some(0), "stderr={}", stderr(&o));
    assert_eq!(stdout(&o).trim(), "refused");
    // Depth-1/2 spawns still work: proven indirectly by the real chain in
    // test_timeout_kills_whole_tree / test_priority_job_wide.
}

fn test_command_not_found_clear_error() {
    // Bare (pathless) name that exists nowhere on PATH: wnice must print the
    // clear cwd-safe resolution error and exit 127.
    let bare = format!("zz-no-such-cmd-{}", std::process::id());
    let o = Command::new(env!("CARGO_BIN_EXE_wnice"))
        .creation_flags(CREATE_NO_WINDOW)
        .arg(&bare)
        .output()
        .expect("spawn wnice");
    assert_eq!(o.status.code(), Some(127), "stderr={}", stderr(&o));
    assert!(
        stderr(&o).contains("command not found"),
        "stderr={}",
        stderr(&o)
    );
}

fn test_cwd_exe_is_never_picked() {
    // A same-named .exe in the CURRENT directory must be ignored: path_dirs
    // drops ""/"." entries, so resolution fails with 127, not runs the file.
    let dir = unique_temp("cwdevil");
    let evil = format!("zz-cwd-evil-{}.exe", std::process::id());
    std::fs::write(dir.join(&evil), b"MZRubbishNotAnExecutable").expect("write evil exe");
    let o = Command::new(env!("CARGO_BIN_EXE_wnice"))
        .creation_flags(CREATE_NO_WINDOW)
        .current_dir(&dir)
        .arg(format!("zz-cwd-evil-{}", std::process::id()))
        .output()
        .expect("spawn wnice");
    assert_eq!(o.status.code(), Some(127), "stderr={}", stderr(&o));
    assert!(
        stderr(&o).contains("command not found"),
        "stderr={}",
        stderr(&o)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------ main ----

/// A copy of wnice.exe named `<name>.exe` (legacy frontends dispatch on argv[0]).
fn legacy_exe(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wnice-legacy-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("legacy dir");
    let exe = dir.join(format!("{name}.exe"));
    if !exe.exists() {
        std::fs::copy(env!("CARGO_BIN_EXE_wnice"), &exe).expect("copy wnice.exe");
    }
    exe
}

fn legacy_probe(name: &str, args: &[&str], mode: &str, probe_args: &[&str]) -> Output {
    check_probe_depth();
    Command::new(legacy_exe(name))
        .creation_flags(CREATE_NO_WINDOW)
        .args(args)
        .arg(probe_exe())
        .args(probe_args)
        .env("WNICE_PROBE", mode)
        .env("WNICE_PROBE_ARGS", probe_args.join(""))
        .env("WNICE_PROBE_DEPTH", (probe_depth() + 1).to_string())
        .output()
        .expect("spawn legacy frontend")
}

fn test_legacy_capc_sets_cpu_cap() {
    let o = legacy_probe("capc", &["50"], "jobinfo", &[]);
    assert_eq!(o.status.code(), Some(0), "stderr={}", stderr(&o));
    let cpurate: u32 = last_line(&o)
        .split_whitespace()
        .find_map(|t| t.strip_prefix("cpurate=").and_then(|v| v.parse().ok()))
        .expect("cpurate field");
    assert_eq!(cpurate, 5000, "capc 50 must be a 50% hard cap");
}

fn test_legacy_caps_timeout_and_exit_codes() {
    let o = legacy_probe("caps", &["0.5"], "sleep", &["20000"]);
    assert_eq!(o.status.code(), Some(124), "stderr={}", stderr(&o));
    let o = legacy_probe("capn", &["5"], "exit", &["7"]);
    assert_eq!(o.status.code(), Some(7), "stderr={}", stderr(&o));
}

fn test_legacy_usage_errors() {
    let o = Command::new(legacy_exe("idle"))
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .expect("spawn idle");
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(
        stderr(&o).lines().next(),
        Some("usage: idle <command> [args...]")
    );
    assert!(stderr(&o).contains("Run 'idle --help'"), "{}", stderr(&o));
    let o = Command::new(legacy_exe("capc"))
        .creation_flags(CREATE_NO_WINDOW)
        .args(["101", "cmd"])
        .output()
        .expect("spawn capc");
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(
        stderr(&o).lines().next(),
        Some("usage: capc <percent 1-100> <command> [args...]")
    );
}

fn test_cy_cx_prefix_agent_flags() {
    // A fake claude.exe / codex.exe (copies of the probe) on a private PATH.
    let bin = std::env::temp_dir().join(format!("wnice-agents-{}", std::process::id()));
    std::fs::create_dir_all(&bin).expect("agents dir");
    for (tool, agent, flag) in [
        ("cy", "claude", "--dangerously-skip-permissions"),
        ("cx", "codex", "--dangerously-bypass-approvals-and-sandbox"),
    ] {
        std::fs::copy(probe_exe(), bin.join(format!("{agent}.exe"))).expect("fake agent");
        check_probe_depth();
        let o = Command::new(legacy_exe(tool))
            .creation_flags(CREATE_NO_WINDOW)
            .args(["x y", "-p"])
            .env("PATH", &bin)
            .env("WNICE_PROBE", "echo")
            .env("WNICE_PROBE_DEPTH", (probe_depth() + 1).to_string())
            .output()
            .expect("spawn agent frontend");
        assert_eq!(o.status.code(), Some(0), "{tool}: stderr={}", stderr(&o));
        assert_eq!(last_line(&o), [flag, "x y", "-p"].join(""), "{tool}");
    }
    let _ = std::fs::remove_dir_all(&bin);
}

fn test_help_exits_zero_with_examples() {
    for (tool, flag) in [
        ("wnice", "--help"),
        ("capc", "-h"),
        ("idle", "/?"),
        ("admin", "--help"),
    ] {
        let exe = if tool == "wnice" {
            PathBuf::from(env!("CARGO_BIN_EXE_wnice"))
        } else {
            legacy_exe(tool)
        };
        let o = Command::new(exe)
            .creation_flags(CREATE_NO_WINDOW)
            .arg(flag)
            .output()
            .expect("spawn help");
        assert_eq!(o.status.code(), Some(0), "{tool}: {}", stderr(&o));
        let out = stdout(&o);
        assert!(out.starts_with(&format!("{tool} - ")), "{tool}: {out}");
        assert!(out.contains("examples:"), "{tool}");
    }
}

const TESTS: &[(&str, fn())] = &[
    (
        "help_exits_zero_with_examples",
        test_help_exits_zero_with_examples,
    ),
    ("cy_cx_prefix_agent_flags", test_cy_cx_prefix_agent_flags),
    ("legacy_capc_sets_cpu_cap", test_legacy_capc_sets_cpu_cap),
    (
        "legacy_caps_timeout_and_exit_codes",
        test_legacy_caps_timeout_and_exit_codes,
    ),
    ("legacy_usage_errors", test_legacy_usage_errors),
    ("exit_code_propagation", test_exit_code_propagation),
    ("negative_exit_code", test_negative_exit_code),
    ("timeout_kills_whole_tree", test_timeout_kills_whole_tree),
    ("priority_job_wide", test_priority_job_wide),
    (
        "priority_fallback_note_or_elevated",
        test_priority_fallback_note_or_elevated,
    ),
    ("affinity_single_cpu", test_affinity_single_cpu),
    ("memory_limit_enforced", test_memory_limit_enforced),
    ("procs_limit", test_procs_limit),
    (
        "daemon_survives_wrapper_exit",
        test_daemon_survives_wrapper_exit,
    ),
    ("no_shell_layer_between", test_no_shell_layer_between),
    (
        "bat_target_with_space_in_path",
        test_bat_target_with_space_in_path,
    ),
    (
        "bat_percent_argument_refused",
        test_bat_percent_argument_refused,
    ),
    (
        "cpu_rate_cap_visible_to_probe",
        test_cpu_rate_cap_visible_to_probe,
    ),
    ("version_and_help", test_version_and_help),
    (
        "post_double_dash_tokens_verbatim",
        test_post_double_dash_tokens_verbatim,
    ),
    (
        "depth_guard_refuses_at_max",
        test_depth_guard_refuses_at_max,
    ),
    (
        "command_not_found_clear_error",
        test_command_not_found_clear_error,
    ),
    ("cwd_exe_is_never_picked", test_cwd_exe_is_never_picked),
];

fn main() {
    // Probe dispatch MUST come first: when WNICE_PROBE is set this process IS
    // a probe, not a test runner. It runs ONLY the requested probe mode and
    // exits — no test body may execute here. (With libtest this binary ran
    // all 20 tests in parallel inside every probe process, each spawning
    // more wnice/probe processes -> recursive multiplication / process flood.
    // harness = false makes that structurally impossible.)
    if let Ok(mode) = std::env::var("WNICE_PROBE") {
        probe_main(&mode); // diverges: never runs the test loop below
    }

    let mut failed: Vec<&str> = Vec::new();
    let mut skipped: Vec<(String, String)> = Vec::new();
    for (name, t) in TESTS {
        // Tests use assert!/expect (panics). catch_unwind keeps a panicking
        // test from aborting the whole run: mark FAILED and continue.
        // Every test waits for the processes it spawns via .output() /
        // .wait_with_output(), so a panic cannot orphan a child mid-run.
        // Skips use the SKIP_REASON thread-local (never panic), read+cleared
        // after each test.
        match std::panic::catch_unwind(t) {
            Ok(()) => {
                if let Some(reason) = SKIP_REASON.with(|r| r.borrow_mut().take()) {
                    println!("skipped {name}: {reason}");
                    skipped.push((name.to_string(), reason));
                } else {
                    println!("ok {name}");
                }
            }
            Err(_) => {
                println!("FAILED {name}");
                failed.push(name);
            }
        }
    }
    let _ = std::fs::remove_dir_all(
        std::env::temp_dir().join(format!("wnice-legacy-{}", std::process::id())),
    );
    // On CI a skip counts as a failure: environment-dependent skips hide
    // real regressions in the environments that matter most.
    let on_ci = std::env::var("CI").map(|v| v == "true").unwrap_or(false);
    let mut ci_skip_failures: Vec<String> = Vec::new();
    if on_ci {
        for (name, reason) in &skipped {
            println!("FAILED {name} (CI forbids skip: {reason})");
            ci_skip_failures.push(name.clone());
        }
    }
    let failed_total = failed.len() + ci_skip_failures.len();
    println!(
        "{}/{} passed, {} skipped{}",
        TESTS.len() - skipped.len() - failed.len(),
        TESTS.len(),
        skipped.len() - ci_skip_failures.len(),
        if failed_total == 0 {
            String::new()
        } else {
            let mut names: Vec<&str> = failed
                .iter()
                .copied()
                .chain(ci_skip_failures.iter().map(|s| s.as_str()))
                .collect();
            names.sort_unstable();
            names.dedup();
            format!("; failed: {}", names.join(", "))
        }
    );
    if failed_total > 0 {
        std::process::exit(1);
    }
}
