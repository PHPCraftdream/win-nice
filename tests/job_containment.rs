//! Job Object containment (harness = false): the four guarantees the wrapper's
//! job carries today. Every helper is observed through a retained process
//! handle and released by an event or a closed pipe — never by a PID looked up
//! after the fact and never by a sleep; the only time-based budget is SAFETY,
//! whose sole job is to turn a hang into a clear failure. Probes are THIS
//! binary re-executed with WNICE_PROBE set (tests/probe_support.rs).
//!
//! WHY harness = false: a probe process must run only the probe; under libtest
//! it would run the test bodies too and multiply processes recursively.

#![cfg(windows)]

#[allow(dead_code)] // the probe module carries modes this suite does not use
mod probe_support;
use probe_support::*;

use std::io::{BufRead, BufReader};
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, STILL_ACTIVE};
use windows_sys::Win32::System::JobObjects::{
    JOB_OBJECT_CPU_RATE_CONTROL_ENABLE, JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
    JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
    JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK,
};
use windows_sys::Win32::System::Threading::TerminateProcess;

/// Hang breaker only: nothing below is ordered by time.
const SAFETY: Duration = Duration::from_secs(120);
/// ERROR_ACCESS_DENIED: what CreateProcessW returns for a breakaway the job does not grant.
const ACCESS_DENIED: u32 = 5;
/// ERROR_NOT_ENOUGH_QUOTA: CreateProcess refused by the job's active-process limit.
const NOT_ENOUGH_QUOTA: i32 = 1816;

static NEXT: AtomicUsize = AtomicUsize::new(0);

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

fn field(line: &str, key: &str) -> Option<u32> {
    let pat = format!("{key}=");
    line.split_whitespace()
        .find_map(|t| t.strip_prefix(&pat).and_then(|v| v.parse().ok()))
}

fn need_field(line: &str, key: &str) -> u32 {
    field(line, key).unwrap_or_else(|| panic!("missing `{key}=` in {line:?}"))
}

fn need_i32(line: &str, key: &str) -> i32 {
    let pat = format!("{key}=");
    line.split_whitespace()
        .find_map(|t| t.strip_prefix(&pat).and_then(|v| v.parse().ok()))
        .unwrap_or_else(|| panic!("missing `{key}=` in {line:?}"))
}

/// Terminates the process behind a retained handle on drop — never by PID.
struct HandleGuard(HANDLE);

impl Drop for HandleGuard {
    fn drop(&mut self) {
        if handle_exit_code(self.0) == Some(STILL_ACTIVE as u32) {
            unsafe { TerminateProcess(self.0, 1) };
        }
        unsafe { CloseHandle(self.0) };
    }
}

/// Signals the release event on drop, so no helper can outlive the test.
struct EventGuard(HANDLE);

impl Drop for EventGuard {
    fn drop(&mut self) {
        set_event(self.0);
        unsafe { CloseHandle(self.0) };
    }
}

/// The wrapper under test: its stdout is streamed line by line so the test can
/// synchronise on events, and it is killed and reaped on EVERY path.
struct Wrapper {
    child: Child,
    lines: Receiver<String>,
}

impl Wrapper {
    fn spawn(args: &[&str], mode: &str, envs: &[(&str, String)]) -> Wrapper {
        check_probe_depth();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_wnice"));
        cmd.creation_flags(CREATE_NO_WINDOW)
            .args(args)
            .arg("--")
            .arg(probe_exe())
            .env("WNICE_PROBE", mode)
            .env("WNICE_PROBE_ARGS", "")
            .env("WNICE_PROBE_DEPTH", (probe_depth() + 1).to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("spawn wnice");
        let out = child.stdout.take().expect("piped stdout");
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Wrapper { child, lines }
    }

    /// Next stdout line; waiting past SAFETY means the tree hung.
    fn next_line(&self, what: &str) -> String {
        self.lines
            .recv_timeout(SAFETY)
            .unwrap_or_else(|_| panic!("{what}: no probe line within {SAFETY:?}"))
    }

    fn wait_code(&mut self) -> Option<i32> {
        self.child.wait().ok().and_then(|s| s.code())
    }
}

impl Drop for Wrapper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ------------------------------------------------------------------- tests

fn test_no_breakaway() {
    // Breaks if JOB_OBJECT_LIMIT_BREAKAWAY_OK is added in src/limits.rs::compose: the explicit request then succeeds.
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
        need_i32(&last, "failed"),
        0,
        "control: none may fail: {last}"
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
        need_i32(&last, "failed"),
        NOT_ENOUGH_QUOTA,
        "the outer ceiling must refuse the third spawn with a quota error: {last}"
    );
}

fn test_membership_survives_parent_exit() {
    // Breaks if the launcher stops assigning the child to the job (src/run/mod.rs): injob becomes 0.
    let name = format!(
        "Local\\wnice-job-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let h_event = create_event(&name);
    assert!(!h_event.is_null(), "CreateEvent({name}) failed");
    let _release = EventGuard(h_event);

    let mut w = Wrapper::spawn(
        &["-s", "10"],
        "orphan-root",
        &[
            ("WNICE_TEST_PID", std::process::id().to_string()),
            ("WNICE_TEST_EVENT", name),
        ],
    );
    let ready = w.next_line("orphan-root readiness");
    assert_eq!(
        need_i32(&ready, "intermediate_exit"),
        0,
        "the intermediate parent must have exited cleanly: {ready}"
    );
    assert_eq!(
        need_field(&ready, "injob"),
        1,
        "the orphaned grandchild must still be in the wrapper's job: {ready}"
    );
    assert_eq!(
        need_field(&ready, "alive"),
        1,
        "the grandchild must be alive while its parent is already gone: {ready}"
    );
    let handle = need_field(&ready, "test_handle") as usize as HANDLE;
    let _grand = HandleGuard(handle);
    assert_eq!(
        handle_exit_code(handle),
        Some(STILL_ACTIVE as u32),
        "the grandchild must still be running: {ready}"
    );

    assert_eq!(
        w.wait_code(),
        Some(124),
        "the wrapper's own timeout must end the run"
    );
    assert_eq!(
        wait_handle(handle, SAFETY.as_millis() as u32),
        0,
        "the timeout kill never reached the orphaned grandchild"
    );
    assert_eq!(
        handle_exit_code(handle),
        Some(124),
        "the grandchild must die from the job's own timeout kill"
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

// -------------------------------------------------------------------- main --

const TESTS: &[(&str, fn())] = &[
    ("no_breakaway", test_no_breakaway),
    (
        "nested_job_does_not_loosen_outer",
        test_nested_job_does_not_loosen_outer,
    ),
    (
        "membership_survives_parent_exit",
        test_membership_survives_parent_exit,
    ),
    ("job_handle_not_inherited", test_job_handle_not_inherited),
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
