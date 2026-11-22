//! The `wnice --jobs` report, end to end (harness = false): a real capped tree
//! is started, the report is asked to describe it, and the row that names that
//! very tree must carry the cap it was started with. The tree is then ended
//! deterministically and its exit code checked.
//!
//! Coordination is pipe-only. The probe announces readiness on the inherited
//! stdout pipe and then blocks on stdin, so the test needs no sleep, no timer
//! and no polling to know the tree is up — and closing stdin is what ends it.
//! Probes are THIS binary re-executed with WNICE_PROBE set (tests/probe_support/).
//!
//! WHY harness = false: a probe process must run only the probe; under libtest
//! it would run the test bodies too and multiply processes recursively.

#![cfg(windows)]

#[allow(dead_code)] // the probe module carries modes this suite does not use
mod probe_support;
use probe_support::*;

use std::io::{BufRead, BufReader, Read};
use std::os::windows::process::CommandExt;
use std::process::{Child, ChildStdin, Command, Output, Stdio};

// ------------------------------------------------------------- test helpers

/// One blocking run of the wrapper, no probe child and no stdin.
fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wnice"))
        .creation_flags(CREATE_NO_WINDOW)
        .args(args)
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

/// The capped tree under test, owned through the wrapper's process handle (the
/// numeric pid is never used to signal it). The guard runs on every exit path,
/// so a panic cannot leave a capped probe blocked on a pipe forever.
struct Tree {
    child: Child,
    stdin: Option<ChildStdin>,
}

impl Tree {
    /// Close the probe's only blocking input, then reap the wrapper. Drop runs
    /// afterwards and finds both already gone, so it stays a no-op.
    fn end(mut self) -> Option<i32> {
        drop(self.stdin.take());
        self.child.wait().ok()?.code()
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        drop(self.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Whatever stderr the wrapper produced. Only called once its stdout reached
/// EOF, which means the probe is gone too, so the read cannot block.
fn stderr_tail(child: &mut Child) -> String {
    let Some(mut err) = child.stderr.take() else {
        return String::new();
    };
    let mut buf = Vec::new();
    let _ = err.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf).into_owned();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return String::new();
    }
    format!(
        "; wrapper stderr: {}",
        lines[lines.len().saturating_sub(5)..].join(" | ")
    )
}

/// The JOBS data row whose owner column is `pid`.
///
/// Only the lines between the `JOBS:` header and the blank line that closes
/// that table are scanned: the PROCESSES table also starts its rows with a pid,
/// so a search anywhere else could match the wrong process.
fn jobs_row(text: &str, pid: u32) -> String {
    let Some(jobs_line) = text
        .lines()
        .position(|l| l.trim_start().starts_with("JOBS:"))
    else {
        panic!("no `JOBS:` line in the report:\n{text}");
    };
    for line in text.lines().skip(jobs_line + 1) {
        if line.trim().is_empty() {
            break;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 2 || fields[0].parse::<usize>().is_err() {
            continue; // the column header row
        }
        if fields[1].parse::<u32>() == Ok(pid) {
            return line.to_string();
        }
    }
    panic!("no JOBS row owned by pid {pid} in the report:\n{text}");
}

// ------------------------------------------------------------------- tests

fn test_jobs_report_lists_a_real_capped_tree() {
    // Breaks if the report stops attributing a capped job to its owner, or
    // reports `-c 20` as anything but a hard 20%-of-all-cores cap.
    check_probe_depth();
    let mut child = Command::new(env!("CARGO_BIN_EXE_wnice"))
        .creation_flags(CREATE_NO_WINDOW)
        .args(["-c", "20", "--"])
        .arg(probe_exe())
        .env("WNICE_PROBE", "ready-wait-eof")
        .env("WNICE_PROBE_DEPTH", (probe_depth() + 1).to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn wnice");
    let stdin = child.stdin.take();
    // Hand the tree to the guard before anything else can fail.
    let mut tree = Tree { child, stdin };

    let mut probe_out = BufReader::new(tree.child.stdout.take().expect("piped stdout"));
    let mut line = String::new();
    let n = probe_out
        .read_line(&mut line)
        .expect("read the probe's ready line");
    assert!(
        n > 0,
        "the wrapper died before the probe reported ready; line {line:?}{}",
        stderr_tail(&mut tree.child)
    );
    assert_eq!(line.trim(), "ready", "probe announced {line:?}");

    let report = run(&["--jobs"]);
    let code = report.status.code();
    assert_eq!(
        code,
        Some(0),
        "wnice --jobs failed: stderr={}",
        stderr(&report)
    );
    let text = stdout(&report);
    for want in ["JOBS:", "PROCESSES:", "owner processes not inspected:"] {
        assert!(text.contains(want), "report lacks {want:?}:\n{text}");
    }

    let row = jobs_row(&text, tree.child.id());
    assert!(
        row.contains("wnice.exe"),
        "the row must name the owning wrapper image: {row:?}"
    );
    assert!(
        row.contains("20.0% all"),
        "the -c 20 cap must be reported as percent of all cores: {row:?}"
    );
    assert!(
        row.contains("hard"),
        "the -c 20 cap must be marked hard: {row:?}"
    );

    // Closing stdin lets the probe return 0, and the wrapper exits with it.
    let code = tree.end();
    assert_eq!(code, Some(0), "the capped tree must exit cleanly");
}

fn test_jobs_flag_usage_errors() {
    // `--jobs` is valid only as the sole argument: any command, option,
    // help/version flag or repeat is a misuse reported on stderr with exit 2
    // and no usage block. It is decided before any other argument is
    // interpreted, so even an invalid neighbour yields the conflict.
    for args in [
        vec!["--jobs", "cmd"],
        vec!["--jobs", "-c", "10", "cmd"],
        vec!["-c", "10", "--jobs"],
        vec!["--jobs", "--boost"],
        vec!["--jobs", "--help"],
        vec!["--jobs", "-h"],
        vec!["--jobs", "--version"],
        vec!["--jobs", "-V"],
        vec!["--jobs", "--jobs"],
        vec!["--jobs", "-c", "0"],
        vec!["--jobs", "--"],
        vec!["--jobs", "--bogus"],
        vec!["--jobs", "-c"],
        vec!["-V", "--jobs"],
        vec!["--jobs", "idle"],
        vec!["cmd", "--jobs"],
    ] {
        let o = run(&args);
        let code = o.status.code();
        assert_eq!(
            code,
            Some(2),
            "args {args:?} must exit 2: stderr={}",
            stderr(&o)
        );
        assert!(
            o.stdout.is_empty(),
            "args {args:?} must print nothing on stdout: {}",
            stdout(&o)
        );
        let err = stderr(&o);
        assert!(
            err.contains("--jobs takes no command and no other options"),
            "args {args:?} must name the conflict: {err}"
        );
        assert!(
            !err.contains("usage:"),
            "args {args:?} must not print the usage block: {err}"
        );
    }

    let o = run(&["--jobs"]);
    let code = o.status.code();
    assert_eq!(
        code,
        Some(0),
        "a bare --jobs must succeed: stderr={}",
        stderr(&o)
    );
    let out = stdout(&o);
    assert!(
        out.starts_with("JOBS: "),
        "the report must open with the JOBS header: {out}"
    );
    assert!(
        out.contains("PROCESSES: "),
        "the report must carry the PROCESSES table: {out}"
    );
    assert!(
        out.contains("owner processes not inspected:"),
        "the report must carry the footer: {out}"
    );
    assert!(
        o.stderr.is_empty(),
        "a bare --jobs must not write stderr: {}",
        stderr(&o)
    );
}

// -------------------------------------------------------------------- main --

const TESTS: &[(&str, fn())] = &[
    (
        "jobs_report_lists_a_real_capped_tree",
        test_jobs_report_lists_a_real_capped_tree,
    ),
    ("jobs_flag_usage_errors", test_jobs_flag_usage_errors),
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
