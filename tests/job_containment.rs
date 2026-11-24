//! Job Object containment (harness = false): the three guarantees the wrapper's
//! job carries today — the job grants no breakaway, a nested job cannot loosen
//! the outer one, and the job handle is not inherited by the child. Every probe
//! line is observed through the wrapper's own stdout for the legacy regression
//! probes; new pool probes execute this binary directly. Probes are THIS binary
//! re-executed with WNICE_PROBE set (tests/probe_support/).
//!
//! WHY harness = false: a probe process must run only the probe; under libtest
//! it would run the test bodies too and multiply processes recursively.

#![cfg(windows)]

#[allow(dead_code)] // the probe module carries modes this suite does not use
mod probe_support;
use probe_support::*;

use std::io::Read;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Output, Stdio};
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

use windows_sys::Win32::System::JobObjects::{
    JOB_OBJECT_CPU_RATE_CONTROL_ENABLE, JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
    JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
    JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK,
};

/// ERROR_ACCESS_DENIED: what CreateProcessW returns for a breakaway the job does not grant.
const ACCESS_DENIED: u32 = 5;
/// ERROR_NOT_ENOUGH_QUOTA: CreateProcess refused by the job's active-process limit.
const NOT_ENOUGH_QUOTA: i32 = 1816;

fn field(line: &str, key: &str) -> Option<u32> {
    let pat = format!("{key}=");
    line.split_whitespace()
        .find_map(|t| t.strip_prefix(&pat).and_then(|v| v.parse().ok()))
}

fn need_field(line: &str, key: &str) -> u32 {
    field(line, key).unwrap_or_else(|| panic!("missing `{key}=` in {line:?}"))
}

/// `failed=<code>`, or None for the probe's `failed=none`.
fn failed_of(line: &str) -> Option<i32> {
    let v = line
        .split_whitespace()
        .find_map(|t| t.strip_prefix("failed="))
        .unwrap_or_else(|| panic!("missing `failed=` in {line:?}"));
    if v == "none" {
        None
    } else {
        Some(
            v.parse()
                .unwrap_or_else(|_| panic!("unexpected failed= value in {line:?}")),
        )
    }
}

// ------------------------------------------------------------------- tests

fn test_no_breakaway() {
    // Breaks if JOB_OBJECT_LIMIT_BREAKAWAY_OK is added in src/limits.rs::compose (the flag assertion fails).
    let o = wnice(&["-c", "50", "-s", "30"], "jobinfo", &[]);
    assert_eq!(o.status.code(), Some(0), "stderr={}", stderr(&o));
    let line = last_line(&o);
    assert_eq!(
        need_field(&line, "cpurate"),
        5_000,
        "the queried job is not wnice's: {line}"
    );
    let flags = need_field(&line, "limitflags");
    assert_eq!(
        flags & (JOB_OBJECT_LIMIT_BREAKAWAY_OK | JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK),
        0,
        "kernel reports breakaway granted while the tree runs: {line}"
    );

    let o = wnice(&["-c", "50", "-s", "30"], "breakaway", &[]);
    assert_eq!(o.status.code(), Some(0), "stderr={}", stderr(&o));
    let out = stdout(&o);
    let job = out
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("job "))
        .unwrap_or_else(|| panic!("no `job ` line in stdout: {out}"));
    assert_eq!(
        need_field(job, "cpurate"),
        5_000,
        "the probe is not inside wnice's job: {job}"
    );
    assert_eq!(
        last_line(&o),
        format!("refused {ACCESS_DENIED}"),
        "CREATE_BREAKAWAY_FROM_JOB inside the job"
    );
}

fn test_nested_job_does_not_loosen_outer() {
    // Breaks if compose stops setting JOB_OBJECT_LIMIT_ACTIVE_PROCESS for -n: all 8 spawns succeed.
    let o = wnice(&["-n", "4", "-s", "30"], "innerjob", &["3"]);
    assert_eq!(o.status.code(), Some(0), "stderr={}", stderr(&o));
    let last = last_line(&o);
    assert_eq!(need_field(&last, "spawned"), 3, "control: {last}");
    assert_eq!(need_field(&last, "alive"), 3, "control: {last}");
    assert_eq!(
        failed_of(&last),
        None,
        "control: nothing may be refused: {last}"
    );

    let o = wnice(&["-n", "3", "-s", "30"], "innerjob", &["8"]);
    assert_eq!(o.status.code(), Some(0), "stderr={}", stderr(&o));
    let out = stdout(&o);
    let inner = out
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("inner "))
        .unwrap_or_else(|| panic!("no `inner ` line in stdout: {out}"));
    assert_eq!(
        need_field(inner, "limitflags") & JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
        0,
        "inner job must carry no process ceiling: {inner}"
    );
    assert_eq!(
        need_field(inner, "cpuctrl"),
        JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
        "inner job must be a hard CPU cap: {inner}"
    );
    assert_eq!(
        need_field(inner, "cpurate"),
        10_000,
        "inner job must allow 100%: {inner}"
    );

    let last = last_line(&o);
    assert_eq!(
        need_field(&last, "spawned"),
        2,
        "root + 2 children is the outer ceiling of 3: {last}"
    );
    assert_eq!(
        need_field(&last, "alive"),
        2,
        "the accepted children must still be alive: {last}"
    );
    assert_eq!(
        failed_of(&last),
        Some(NOT_ENOUGH_QUOTA),
        "the outer ceiling must refuse the third spawn with a quota error: {last}"
    );
}

fn test_job_handle_not_inherited() {
    // Breaks if the job handle becomes inheritable (CreateJobObjectW with bInheritHandle=TRUE): jobhandles > own.
    let o = wnice(&["-s", "30"], "handleaudit", &[]);
    assert_eq!(o.status.code(), Some(0), "stderr={}", stderr(&o));
    let last = last_line(&o);
    let own = need_field(&last, "own");
    assert_eq!(own, 3, "probe must have created three job handles: {last}");
    assert_eq!(
        need_field(&last, "jobhandles"),
        own,
        "an inherited job handle is visible to the probe: {last}"
    );
    assert_eq!(
        need_field(&last, "injob"),
        0,
        "a handle the probe is a member through was inherited from the wrapper: {last}"
    );
}

// ------------------------------------------------------------- test helpers

/// One blocking run of the wrapper with a probe child (`WNICE_PROBE`), for the
/// tests that need no cross-process coordination.
fn wnice(args: &[&str], mode: &str, probe_args: &[&str]) -> Output {
    check_probe_depth();
    Command::new(env!("CARGO_BIN_EXE_wnice"))
        .creation_flags(CREATE_NO_WINDOW)
        .args(args)
        .arg("--")
        .arg(probe_exe())
        .args(probe_args)
        .env("WNICE_PROBE", mode)
        .env("WNICE_PROBE_ARGS", probe_args.join("\u{1f}"))
        .env("WNICE_PROBE_DEPTH", (probe_depth() + 1).to_string())
        .stdin(Stdio::null())
        .output()
        .expect("spawn wnice")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn last_line(o: &Output) -> String {
    stdout(o)
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string()
}
fn test_pool_membership_probes() {
    let base = "wnice-pool";
    let run = |mode: &str, names: &[&str]| {
        let mut guard = ChildGuard(spawn_probe_piped(mode, names));
        let mut output = String::new();
        guard
            .0
            .stdout
            .take()
            .expect("probe stdout")
            .read_to_string(&mut output)
            .expect("read probe");
        let status = guard.0.wait().expect("wait probe");
        assert!(status.success(), "{mode}: {output}");
        output
    };
    let print_pool = |case, output: &str| {
        for line in output.lines() {
            println!("pool {case}: {line}");
        }
    };
    let self_join = run("pool-self", &[base]);
    print_pool("self_join", &self_join);
    assert!(
        self_join.contains("assign_x=ok assign_p=ok in_x=1 in_p=1"),
        "{self_join}"
    );
    assert!(self_join.contains("query_x=ok query_p=ok"), "{self_join}");

    let sibling = run("pool-sibling", &[base]);
    print_pool("sibling", &sibling);
    let code: u32 = sibling
        .split_whitespace()
        .find_map(|t| {
            t.strip_prefix("sibling_error=")
                .and_then(|v| v.parse().ok())
        })
        .expect("numeric sibling Win32 error");
    println!(
        "pool sibling: sibling_error={code}; hypothesis/expected Windows result is 5 (ERROR_ACCESS_DENIED), unmeasured locally"
    );
    assert!(
        sibling.contains("assign_x=ok"),
        "sibling_error={code}: child must join X2 before trying P: {sibling}"
    );
    let expected_child_p = u32::from(code == 0);
    assert_eq!(
        field(&sibling, "child_x"),
        Some(1),
        "sibling_error={code}: child must really join X2: {sibling}"
    );
    assert_eq!(
        field(&sibling, "child_x1"),
        Some(0),
        "sibling_error={code}: child in X2 must not be a member of parent X1: {sibling}"
    );
    assert_eq!(
        field(&sibling, "child_p"),
        Some(expected_child_p),
        "sibling_error={code}: P membership must match AssignProcessToJobObject result: {sibling}"
    );
    assert_eq!(
        field(&sibling, "parent_x"),
        Some(1),
        "sibling_error={code}: parent must remain in X1: {sibling}"
    );
    assert_eq!(
        field(&sibling, "parent_p"),
        Some(1),
        "sibling_error={code}: parent must remain in P: {sibling}"
    );

    let nested = run("pool-nested", &[base]);
    print_pool("nested", &nested);
    assert!(nested.contains("child_x=1 child_p=1 child_c=1"), "{nested}");
    assert!(nested.contains("query_c=ok"), "{nested}");
    assert!(nested.contains("cpuctrl=5 cpurate=1000"), "{nested}");

    let shared = run("pool-shared", &[base]);
    print_pool("shared", &shared);
    assert!(shared.contains("first_assign_p=ok"), "{shared}");
    assert!(shared.contains("first_x=1 first_p=1"), "{shared}");
    assert!(shared.contains("second_assign_p=ok"), "{shared}");
    assert!(shared.contains("second_x=1 second_p=1"), "{shared}");
}

// -------------------------------------------------------------------- main --

const TESTS: &[(&str, fn())] = &[
    ("no_breakaway", test_no_breakaway),
    (
        "nested_job_does_not_loosen_outer",
        test_nested_job_does_not_loosen_outer,
    ),
    ("job_handle_not_inherited", test_job_handle_not_inherited),
    ("pool_membership_probes", test_pool_membership_probes),
];

fn main() {
    // Probe dispatch MUST come first: with WNICE_PROBE set this process IS a
    // probe and runs only that mode (probe_main diverges).
    if let Ok(mode) = std::env::var("WNICE_PROBE") {
        probe_main(&mode);
    }

    let mut failed: Vec<&str> = Vec::new();
    for (name, t) in TESTS {
        match std::panic::catch_unwind(t) {
            Ok(()) => println!("ok {name}"),
            Err(_) => {
                println!("FAILED {name}");
                failed.push(name);
            }
        }
    }
    println!(
        "{}/{} passed{}",
        TESTS.len() - failed.len(),
        TESTS.len(),
        if failed.is_empty() {
            String::new()
        } else {
            let mut names = failed.clone();
            names.sort_unstable();
            format!("; failed: {}", names.join(", "))
        }
    );
    if !failed.is_empty() {
        std::process::exit(1);
    }
}
