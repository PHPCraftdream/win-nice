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

/// Start a `-c <cap>` wrapper around a blocking probe and wait for its ready
/// line. The returned guard ends the tree on every exit path.
fn start_tree(cap: &str) -> Tree {
    let mut child = Command::new(env!("CARGO_BIN_EXE_wnice"))
        .creation_flags(CREATE_NO_WINDOW)
        .args(["-c", cap, "--"])
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
    tree
}

// ------------------------------------------------------------------- tests

fn test_jobs_report_lists_a_real_capped_tree() {
    // Breaks if the report stops attributing a capped job to its owner, or
    // reports `-c 20` as anything but a hard 20%-of-all-cores cap.
    check_probe_depth();
    let tree = start_tree("20");

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
    // help/version flag or repeat is a misuse reported on stderr with exit 2.
    // It is decided before any other argument is interpreted, so even an
    // invalid neighbour yields the conflict and the message is a single line:
    // no usage block (and no hint line) can ever be printed.
    for args in [
        vec!["--jobs", "cmd"],
        vec!["--jobs", "-c", "10", "cmd"],
        vec!["-c", "10", "--jobs"],
        vec!["--jobs", "--boost"],
        vec!["--jobs", "--wsl"],
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
        assert_eq!(
            err.lines().count(),
            1,
            "args {args:?} must print exactly one stderr line: {err}"
        );
        assert_eq!(
            err.trim_end_matches('\n'),
            "wnice: --jobs takes no command and no other options; run 'wnice --jobs' on its own",
            "args {args:?}"
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

fn test_jobs_report_lists_two_separate_trees() {
    // Two unrelated wrappers hold two different job objects; the report must
    // keep them as two rows, each with its own cap. A report that merged jobs
    // (for example because the kernel object address is hidden from an
    // unprivileged caller) would show one row, or the wrong cap, here.
    check_probe_depth();
    let first = start_tree("20");
    let second = start_tree("30");

    let report = run(&["--jobs"]);
    assert_eq!(
        report.status.code(),
        Some(0),
        "wnice --jobs failed: stderr={}",
        stderr(&report)
    );
    let text = stdout(&report);
    let row_a = jobs_row(&text, first.child.id());
    let row_b = jobs_row(&text, second.child.id());
    assert_ne!(
        row_a, row_b,
        "two jobs must be two rows:
{text}"
    );
    assert!(
        row_a.contains("20.0% all"),
        "first tree: {row_a:?}
{text}"
    );
    assert!(
        row_b.contains("30.0% all"),
        "second tree: {row_b:?}
{text}"
    );
    let id = |row: &str| row.split_whitespace().next().map(str::to_owned);
    assert_ne!(id(&row_a), id(&row_b), "rows must carry distinct ids");

    assert_eq!(first.end(), Some(0));
    assert_eq!(second.end(), Some(0));
}

fn test_jobs_after_the_command_is_an_ordinary_argument() {
    // 0.4.0 passed every argument after the first non-option token to the
    // command verbatim; `--jobs` there must not become the report or an error.
    for args in [
        vec!["cmd", "/c", "echo", "--jobs"],
        vec!["-c", "50", "cmd", "/c", "echo", "--jobs"],
        vec!["--", "cmd", "/c", "echo", "--jobs"],
    ] {
        let o = run(&args);
        assert_eq!(
            o.status.code(),
            Some(0),
            "args {args:?}: stderr={}",
            stderr(&o)
        );
        assert_eq!(stdout(&o).trim(), "--jobs", "args {args:?}");
        assert!(o.stderr.is_empty(), "args {args:?}: {}", stderr(&o));
    }
}

/// The number of wnice jobs a probe reports itself in, started under `wrappers`
/// nested `wnice -c` wrappers (0: started directly by this harness).
fn nesting_seen_through(wrappers: &[&str]) -> usize {
    check_probe_depth();
    let (first, rest) = match wrappers.split_first() {
        Some((first, rest)) => (Some(*first), rest),
        None => (None, wrappers),
    };
    let mut command = match first {
        Some(cap) => {
            let mut c = Command::new(env!("CARGO_BIN_EXE_wnice"));
            c.args(["-c", cap, "--"]);
            for cap in rest {
                c.arg(env!("CARGO_BIN_EXE_wnice")).args(["-c", cap, "--"]);
            }
            c.arg(probe_exe());
            c
        }
        None => Command::new(probe_exe()),
    };
    let o = command
        .creation_flags(CREATE_NO_WINDOW)
        .env("WNICE_PROBE", "nesting-count")
        .env("WNICE_PROBE_DEPTH", (probe_depth() + 1).to_string())
        .stdin(Stdio::null())
        .output()
        .expect("spawn the nesting probe");
    assert_eq!(o.status.code(), Some(0), "stderr={}", stderr(&o));
    let text = stdout(&o);
    let line = text.lines().last().unwrap_or("");
    line.strip_prefix("nesting:")
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no nesting count in {text:?}"))
}

fn test_nesting_count_follows_the_job_chain() {
    // The count behind admin's nested-caller note: every wnice job around the
    // process counts once, and the harness's own surroundings are the baseline
    // (a CI job that no wnice wrapper owns is not counted).
    let base = nesting_seen_through(&[]);
    assert_eq!(nesting_seen_through(&["20"]), base + 1);
    assert_eq!(nesting_seen_through(&["20", "30"]), base + 2);
    assert_eq!(nesting_seen_through(&["20", "30", "40"]), base + 3);
}

// -------------------------------------------------------------------- main --

const TESTS: &[(&str, fn())] = &[
    (
        "jobs_report_lists_a_real_capped_tree",
        test_jobs_report_lists_a_real_capped_tree,
    ),
    ("jobs_flag_usage_errors", test_jobs_flag_usage_errors),
    (
        "jobs_report_lists_two_separate_trees",
        test_jobs_report_lists_two_separate_trees,
    ),
    (
        "jobs_after_the_command_is_an_ordinary_argument",
        test_jobs_after_the_command_is_an_ordinary_argument,
    ),
    (
        "nesting_count_follows_the_job_chain",
        test_nesting_count_follows_the_job_chain,
    ),
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
