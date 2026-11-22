//! Backward-compatibility suite: the published 0.4.0 CLI contract, pinned.
//!
//! Fails when any of these changes: the `--help` text of every tool, every
//! usage and error message, the documented exit codes (124 timeout, 127
//! command not found, 1 wrapper error, exact passthrough including negative
//! NTSTATUS values) and the Job Object lifecycle (limits applied to the whole
//! tree, the tree killed on timeout, a background child surviving a normal
//! exit because KILL_ON_JOB_CLOSE is released then).
//!
//! `--help` and usage texts are committed goldens under `tests/compat/`; see
//! CONTRIBUTING.md, "Compatibility contract", before changing one. The same
//! contracts are checked in CI against the published binary by
//! `scripts/compat-differential.js`.
//!
//! harness = false for the same reason as tests/integration.rs: this binary
//! also runs as a probe child of wnice (WNICE_PROBE), and under libtest such a
//! probe process would re-run every test and multiply processes recursively.
//! Probe modes and their env contract live in tests/probe_support/.

#![cfg(windows)]

#[allow(dead_code)] // the probe module carries modes this suite does not use
mod probe_support;
use probe_support::*;

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::JobObjects::{
    JOB_OBJECT_CPU_RATE_CONTROL_ENABLE, JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use wnice::Sys;

// ------------------------------------------------------------- fixtures ----

/// Every name the multicall binary dispatches on (argv[0] file stem).
const STEMS: [&str; 15] = [
    "wnice",
    "idle",
    "belownormal",
    "abovenormal",
    "high",
    "realtime",
    "capc",
    "capt",
    "capm",
    "caps",
    "capn",
    "cy",
    "cx",
    "admin",
    "uiup",
];

/// Tools with a `--help` of their own. cy/cx forward the flag to the agent they
/// launch, so they have none.
const HELP_STEMS: [&str; 13] = [
    "wnice",
    "idle",
    "belownormal",
    "abovenormal",
    "high",
    "realtime",
    "capc",
    "capt",
    "capm",
    "caps",
    "capn",
    "admin",
    "uiup",
];

const WNICE_HINT: &str = "Run 'wnice --help' for details and examples.\n";

const PERCENT_REFUSAL: &str = "wnice: Refusing to run: argument contains '%' and the target needs the cmd.exe fallback (not a directly-launchable .exe), where '%' can trigger unintended environment-variable expansion. See README's Argument handling section.\n";

const TIMEOUT_1S: &str =
    "wnice: timed out after 1s - job and every process in it were force-killed\n";

const TIMEOUT_2S: &str =
    "wnice: timed out after 2s - job and every process in it were force-killed\n";

/// Daemon-probe env: once the file named by `WNICE_COMPAT_PROBE_JOBINFO_GO`
/// exists, report this process's Job Object limits to the file named by
/// `WNICE_COMPAT_PROBE_JOBINFO_OUT` and stay alive. The test creates the "go"
/// file only after it observed wnice exit, so the report describes the job
/// AFTER the wrapper is gone.
const PROBE_JOBINFO_OUT: &str = "WNICE_COMPAT_PROBE_JOBINFO_OUT";
const PROBE_JOBINFO_GO: &str = "WNICE_COMPAT_PROBE_JOBINFO_GO";

fn compat_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("compat")
}

/// A committed golden. `help/*.txt` holds exactly the bytes `--help` writes,
/// final newline included; `usage/*.txt` holds the usage text plus one newline.
fn golden(rel: &str) -> String {
    let path = compat_dir().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// A usage golden without its single trailing newline: callers compose it into
/// the full stderr message (`wnice: <msg>. <usage>` / `<usage>` + hint).
fn usage(name: &str) -> String {
    let text = golden(&format!("usage/{name}.txt"));
    let stripped = text
        .strip_suffix('\n')
        .unwrap_or_else(|| panic!("usage/{name}.txt must end with a newline"));
    assert!(
        !stripped.ends_with('\n'),
        "usage/{name}.txt must end with exactly one newline"
    );
    stripped.to_string()
}

/// One copy of the built binary per tool name, the layout the installer
/// produces: the tools dispatch on argv[0]'s file stem. Created once per run.
fn tools_dir() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("wnice-compat-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("tool dir");
        for stem in STEMS {
            std::fs::copy(env!("CARGO_BIN_EXE_wnice"), dir.join(format!("{stem}.exe")))
                .expect("stage tool exe");
        }
        dir
    })
}

fn run(stem: &str, args: &[&str]) -> Output {
    Command::new(tools_dir().join(format!("{stem}.exe")))
        .creation_flags(CREATE_NO_WINDOW)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("spawn tool")
}

/// `<tool>.exe <tool_args...> <probe exe> <probe args...>` with the probe env
/// pinned. Extra probe arguments travel via WNICE_PROBE_ARGS, never via argv,
/// so the tool never option-parses them (see tests/integration.rs).
fn tool_probe(stem: &str, tool_args: &[&str], mode: &str, probe_args: &[&str]) -> Output {
    check_probe_depth();
    Command::new(tools_dir().join(format!("{stem}.exe")))
        .creation_flags(CREATE_NO_WINDOW)
        .args(tool_args)
        .arg(probe_exe())
        .env("WNICE_PROBE", mode)
        .env("WNICE_PROBE_ARGS", probe_args.join("\u{1f}"))
        .env("WNICE_PROBE_DEPTH", (probe_depth() + 1).to_string())
        .stdin(Stdio::null())
        .output()
        .expect("spawn probe tool")
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn last_line(o: &Output) -> String {
    out(o)
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn expect_exact(stem: &str, args: &[&str], code: i32, want_out: &str, want_err: &str) {
    let o = run(stem, args);
    assert_eq!(
        o.status.code(),
        Some(code),
        "{stem} {args:?}: stderr={}",
        err(&o)
    );
    assert_eq!(out(&o), want_out, "{stem} {args:?}: stdout");
    assert_eq!(err(&o), want_err, "{stem} {args:?}: stderr");
}

/// Kernel-reported limits of the job this process belongs to, in the same
/// `limitflags=<n> cpuctrl=<n> cpurate=<n>` shape the probe's jobinfo mode
/// prints. `Err` carries the message instead of exiting, so the probe can
/// report exactly which query failed.
fn job_report() -> Result<String, String> {
    use windows_sys::Win32::System::JobObjects::{
        JobObjectCpuRateControlInformation, JobObjectExtendedLimitInformation,
        QueryInformationJobObject, JOBOBJECT_CPU_RATE_CONTROL_INFORMATION,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    };
    unsafe {
        let mut ext: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        let ok = QueryInformationJobObject(
            std::ptr::null_mut(),
            JobObjectExtendedLimitInformation,
            &mut ext as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            std::ptr::null_mut(),
        );
        if ok == 0 {
            return Err("QueryInformationJobObject(extended) failed".to_string());
        }
        let mut cpu: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION = std::mem::zeroed();
        let ok = QueryInformationJobObject(
            std::ptr::null_mut(),
            JobObjectCpuRateControlInformation,
            &mut cpu as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_CPU_RATE_CONTROL_INFORMATION>() as u32,
            std::ptr::null_mut(),
        );
        if ok == 0 {
            return Err("QueryInformationJobObject(cpu-rate) failed".to_string());
        }
        Ok(format!(
            "limitflags={} cpuctrl={} cpurate={}",
            ext.BasicLimitInformation.LimitFlags, cpu.ControlFlags, cpu.Anonymous.CpuRate
        ))
    }
}

fn job_field(report: &str, key: &str) -> u32 {
    report
        .split_whitespace()
        .find_map(|t| {
            t.strip_prefix(&format!("{key}="))
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or_else(|| panic!("{key} missing in {report:?}"))
}

/// Waits for the "go" file, then publishes this process's job limits ATOMICALLY
/// (temp file + rename) and stays alive, so the test can see that the survivor
/// was alive while it reported. Every failure path exits with its own code and a
/// message instead of leaving a half-written report behind.
fn probe_jobinfo_after(out: &str, go: &str) -> ! {
    hand_off_self();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !Path::new(go).exists() {
        if Instant::now() > deadline {
            eprintln!("compat probe: no go file at {go} after 30s");
            std::process::exit(3);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let report = match job_report() {
        Ok(report) => report,
        Err(e) => {
            eprintln!("compat probe: {e}");
            std::process::exit(4);
        }
    };
    // complete=1 is the end marker: a reader that does not see it must not
    // treat the file as data.
    let body = format!("{report} complete=1\n");
    let tmp = format!("{out}.tmp");
    if let Err(e) = std::fs::write(&tmp, &body) {
        eprintln!("compat probe: cannot write {tmp}: {e}");
        std::process::exit(5);
    }
    if let Err(e) = std::fs::rename(&tmp, out) {
        eprintln!("compat probe: cannot publish {out}: {e}");
        std::process::exit(6);
    }
    std::thread::sleep(Duration::from_secs(60));
    std::process::exit(0);
}

/// A complete probe report. Parsed strictly: exactly the four fields
/// `limitflags`, `cpuctrl`, `cpurate` and the trailing `complete=1` end marker,
/// in that order; an unknown, missing, duplicated or reordered field and a
/// missing end marker are errors, never data.
struct JobReport {
    limitflags: u32,
    cpuctrl: u32,
    cpurate: u32,
}

fn parse_job_report(text: &str) -> Result<JobReport, String> {
    const KEYS: [&str; 4] = ["limitflags", "cpuctrl", "cpurate", "complete"];
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.len() != KEYS.len() {
        return Err(format!(
            "expected {} fields, found {} in {text:?}",
            KEYS.len(),
            tokens.len()
        ));
    }
    let mut values = [0u32; KEYS.len()];
    for (i, (token, key)) in tokens.iter().zip(KEYS).enumerate() {
        let (found, value) = token
            .split_once('=')
            .ok_or_else(|| format!("unexpected token {token:?}"))?;
        if found != key {
            return Err(format!("expected {key}=..., found {token:?}"));
        }
        values[i] = value
            .parse()
            .map_err(|_| format!("unparsable value in {token:?}"))?;
    }
    // complete=1 is the end marker: it must be last and must be 1.
    if values[3] != 1 {
        return Err(format!("end marker must be complete=1, found {text:?}"));
    }
    Ok(JobReport {
        limitflags: values[0],
        cpuctrl: values[1],
        cpurate: values[2],
    })
}

/// Waits for a complete, atomically published report. Deliberately loud: an
/// unreadable, partial or marker-less file is retried and never trusted, and a
/// survivor that dies without publishing is named in the panic with the last
/// reason. Nothing here can silently succeed on stale data - the caller's
/// scratch directory is fresh and the file cannot exist before the run starts.
fn wait_for_job_report(path: &Path, survivor: &SurvivorGuard, budget: Duration) -> JobReport {
    let deadline = Instant::now() + budget;
    let mut last_error = String::from("the report file does not exist yet");
    while Instant::now() < deadline {
        match std::fs::read_to_string(path) {
            Ok(text) => match parse_job_report(&text) {
                Ok(report) => return report,
                Err(e) => {
                    last_error = format!(
                        "{} holds an unusable report: {e} (content {text:?})",
                        path.display()
                    );
                }
            },
            Err(e) => last_error = format!("cannot read {}: {e}", path.display()),
        }
        if !survivor.alive() {
            panic!("the survivor exited before publishing a complete report: {last_error}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!(
        "no complete job report at {} within {budget:?}: {last_error}",
        path.display()
    );
}

/// Waits for a survivor to publish its own handle and adopts it. The PID is
/// never used again: observation and termination go through the handle.
fn adopt_survivor(path: &Path) -> SurvivorGuard {
    let deadline = Instant::now() + Duration::from_secs(40);
    while Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(value) = text.trim().parse::<usize>() {
                return SurvivorGuard::adopt(value as HANDLE);
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("no survivor handle at {} within 40s", path.display());
}

/// A scratch directory unique to one test run, created exclusively (an existing
/// path is a panic) and removed on drop. A failing assertion must not leave a
/// "go" file or a report behind that a later run could mistake for its own.
struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new(tag: &str) -> ScratchDir {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path =
            std::env::temp_dir().join(format!("wnice-compat-{tag}-{}-{nanos}", std::process::id()));
        // An existing path would mean someone else's leftovers: refuse it.
        // create_dir (not create_dir_all) makes the check atomic: it fails with
        // AlreadyExists instead of adopting the path in a race.
        if let Err(e) = std::fs::create_dir(&path) {
            panic!("cannot create scratch dir {}: {e}", path.display());
        }
        ScratchDir(path)
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ------------------------------------------------------------------ tests --

fn test_help_goldens() {
    for stem in HELP_STEMS {
        let o = run(stem, &["--help"]);
        assert_eq!(o.status.code(), Some(0), "{stem}: {}", err(&o));
        assert_eq!(err(&o), "", "{stem}: --help must not write to stderr");
        assert_eq!(
            o.stdout,
            golden(&format!("help/{stem}.txt")).into_bytes(),
            "{stem}: --help drifted from tests/compat/help/{stem}.txt"
        );
    }
}

fn test_help_flags() {
    for (stem, flag) in [
        ("wnice", "-h"),
        ("idle", "-h"),
        ("belownormal", "/?"),
        ("high", "-h"),
        ("realtime", "/?"),
        ("capc", "/?"),
        ("capt", "/?"),
        ("capm", "-h"),
        ("caps", "/?"),
        ("capn", "-h"),
        ("admin", "/?"),
        ("uiup", "-h"),
    ] {
        let o = run(stem, &[flag]);
        assert_eq!(o.status.code(), Some(0), "{stem} {flag}: {}", err(&o));
        assert_eq!(
            o.stdout,
            golden(&format!("help/{stem}.txt")).into_bytes(),
            "{stem} {flag} must print the same help as --help"
        );
    }
}

fn test_version_shape() {
    // The number itself belongs to the release, not to the contract; the
    // format `wnice <version>` does.
    let want = format!("wnice {}\n", env!("CARGO_PKG_VERSION"));
    for flag in ["-V", "--version"] {
        let o = run("wnice", &[flag]);
        assert_eq!(o.status.code(), Some(0), "{flag}: {}", err(&o));
        assert_eq!(out(&o), want, "{flag}");
        assert_eq!(err(&o), "", "{flag}");
    }
}

/// (expected message, arguments) for every `wnice` usage error. main.rs wraps
/// the message as `wnice: <msg>. <USAGE>` and appends the help hint, exit 1.
const WNICE_ERRORS: &[(&str, &[&str])] = &[
    ("no command given", &[]),
    ("no command given", &["--"]),
    ("unknown option '--bogus'", &["--bogus"]),
    ("unknown option '-x'", &["-x"]),
    ("unknown option '-pc'", &["-pc", "idle", "cmd"]),
    ("unknown option '--priority=idle'", &["--priority=idle", "cmd"]),
    ("missing value for -p/--priority", &["-p"]),
    ("missing value for -s/--timeout", &["--timeout"]),
    ("invalid -p/--priority value 'bogus'", &["-p", "bogus", "cmd"]),
    ("invalid -c/--cpu value '0'", &["-c", "0", "cmd"]),
    ("invalid -c/--cpu value '101'", &["-c", "101", "cmd"]),
    ("invalid -c/--cpu value '50.5'", &["-c", "50.5", "cmd"]),
    ("invalid -t/--threads value '0'", &["-t", "0", "cmd"]),
    ("invalid -t/--threads value '64'", &["-t", "64", "cmd"]),
    ("invalid -m/--memory <size>", &["-m", "101", "cmd"]),
    ("invalid -m/--memory <size>", &["-m", "50.5", "cmd"]),
    ("invalid -m/--memory <size>", &["-m", "512kb", "cmd"]),
    // The distinct overflow diagnostic (src/cli.rs): too many digits, and a
    // byte value that does not fit in u64.
    (
        "invalid -m/--memory <size>: out of range",
        &["-m", "9999999999999999999999999999999m", "cmd"],
    ),
    (
        "invalid -m/--memory <size>: out of range",
        &["-m", "18446744073709551616m", "cmd"],
    ),
    ("invalid -n/--procs value '0'", &["-n", "0", "cmd"]),
    ("invalid -n/--procs value '-1'", &["-n", "-1", "cmd"]),
    ("invalid -s/--timeout <seconds>", &["-s", "0", "cmd"]),
    ("invalid -s/--timeout <seconds>", &["-s", "0.0009", "cmd"]),
    ("invalid --wsl value (empty distro name)", &["--wsl=", "cmd"]),
    ("--boost requires -p/--priority", &["--boost", "cmd"]),
    (
        "--boost does not apply to --wsl: the guest priority comes from the WSL cgroup (nice/CPUWeight), not from a Windows job",
        &["--wsl", "--boost", "-p", "high", "cmd"],
    ),
];

fn test_wnice_usage_errors() {
    let usage = usage("wnice");
    for (msg, args) in WNICE_ERRORS {
        let want = format!("wnice: {msg}. {usage}\n{}", WNICE_HINT);
        expect_exact("wnice", args, 1, "", &want);
    }
}

fn test_legacy_usage_errors() {
    let hint = |stem: &str| format!("Run '{stem} --help' for details and examples.\n");

    for stem in ["idle", "belownormal", "abovenormal", "high", "realtime"] {
        let want = format!("usage: {stem} <command> [args...]\n{}", hint(stem));
        expect_exact(stem, &[], 1, "", &want);
    }

    let capc = "usage: capc <percent 1-100> <command> [args...]".to_string();
    // Same bound as -t: min(logical processors, 63), read from the same trait
    // method the tools use, so the expectation is machine independent.
    let max_threads = wnice::RealSys.logical_processor_count().clamp(1, 63);
    let capt = format!("usage: capt <thread-count 1-{max_threads}> <command> [args...]");
    let capm = usage("capm");
    let caps = usage("caps");
    let capn = usage("capn");

    let cases: Vec<(&str, &str, Vec<&str>)> = vec![
        ("capc", capc.as_str(), vec![]),
        ("capc", capc.as_str(), vec!["50"]),
        ("capc", capc.as_str(), vec!["0", "cmd"]),
        ("capc", capc.as_str(), vec!["101", "cmd"]),
        ("capc", capc.as_str(), vec!["50.5", "cmd"]),
        ("capt", capt.as_str(), vec![]),
        ("capt", capt.as_str(), vec!["1"]),
        ("capt", capt.as_str(), vec!["0", "cmd"]),
        ("capm", capm.as_str(), vec![]),
        ("capm", capm.as_str(), vec!["50.5", "cmd"]),
        ("capm", capm.as_str(), vec!["25%", "cmd"]),
        ("capm", capm.as_str(), vec!["0m", "cmd"]),
        ("caps", caps.as_str(), vec![]),
        ("caps", caps.as_str(), vec!["0", "cmd"]),
        ("caps", caps.as_str(), vec!["-1", "cmd"]),
        ("caps", caps.as_str(), vec!["1.2.3", "cmd"]),
        ("capn", capn.as_str(), vec![]),
        ("capn", capn.as_str(), vec!["0", "cmd"]),
        ("capn", capn.as_str(), vec!["4294967296", "cmd"]),
    ];
    for (stem, text, args) in cases {
        let want = format!("{text}\n{}", hint(stem));
        expect_exact(stem, &args, 1, "", &want);
    }

    // admin without a command never reaches the elevation path.
    let want = format!("usage: admin <command> [args...]\n{}", hint("admin"));
    expect_exact("admin", &[], 1, "", &want);
}

fn test_all_options_accepted() {
    // Every documented option in one run: real limits, no load.
    let o = tool_probe(
        "wnice",
        &[
            "-p", "idle", "-c", "50", "-t", "1", "-m", "512m", "-n", "10", "-s", "30", "--",
        ],
        "exit",
        &["0"],
    );
    assert_eq!(o.status.code(), Some(0), "stderr={}", err(&o));

    // --boost (with -p) and a priority class spelled in mixed case.
    let o = tool_probe("wnice", &["--boost", "-p", "High"], "exit", &["3"]);
    assert_eq!(o.status.code(), Some(3), "stderr={}", err(&o));

    // --wsl/--wsl=<distro> are parsed and validated here; the error below is
    // raised by the option AFTER them, which proves they were accepted. No WSL
    // run is attempted (there is none on CI).
    let o = run("wnice", &["--wsl", "-c", "0", "cmd"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        err(&o).starts_with("wnice: invalid -c/--cpu value '0'. "),
        "{}",
        err(&o)
    );
    let o = run("wnice", &["--wsl=Ubuntu-24.04", "-t", "64", "cmd"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        err(&o).starts_with("wnice: invalid -t/--threads value '64'. "),
        "{}",
        err(&o)
    );
}

fn test_exit_code_passthrough() {
    for code in ["0", "1", "5", "7", "255"] {
        let o = tool_probe("wnice", &["--"], "exit", &[code]);
        assert_eq!(
            o.status.code(),
            Some(code.parse().unwrap()),
            "wnice -- exit {code}: {}",
            err(&o)
        );
    }
    // Windows exit codes are full 32-bit values: a negative NTSTATUS has to
    // survive the wrapper bit for bit.
    let o = tool_probe("wnice", &["--"], "exit", &["-1"]);
    assert_eq!(
        o.status.code(),
        Some(-1),
        "negative passthrough: {}",
        err(&o)
    );

    for (stem, args, code) in [
        ("idle", &[][..], "5"),
        ("high", &[][..], "7"),
        ("capc", &["50"][..], "3"),
        ("capt", &["1"][..], "4"),
        ("capm", &["90"][..], "6"),
        ("caps", &["30"][..], "8"),
        ("capn", &["10"][..], "9"),
    ] {
        let o = tool_probe(stem, args, "exit", &[code]);
        assert_eq!(
            o.status.code(),
            Some(code.parse().unwrap()),
            "{stem}: {}",
            err(&o)
        );
    }
}

fn test_command_not_found_127() {
    let name = format!("wnice-compat-no-such-{}", std::process::id());
    let want = format!(
        "wnice: '{name}': command not found (searched PATH; the current directory is never searched)\n"
    );
    expect_exact("wnice", &["--", &name], 127, "", &want);
    expect_exact("idle", &[&name], 127, "", &want);
}

fn test_percent_refusal_and_bat_exit_code() {
    let dir = std::env::temp_dir().join(format!("wnice-compat-bat-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("bat dir");
    let bat = dir.join("noop.bat");
    std::fs::write(&bat, "@echo off\r\nexit /b 3\r\n").expect("write bat");
    let bat = bat.to_string_lossy().into_owned();

    expect_exact("wnice", &["--", &bat, "100%"], 1, "", PERCENT_REFUSAL);
    expect_exact("capc", &["50", &bat, "50%"], 1, "", PERCENT_REFUSAL);
    // Without a '%' the same .bat runs through the cmd.exe fallback and its
    // exit code comes through unchanged.
    expect_exact("wnice", &["--", &bat], 3, "", "");
    let _ = std::fs::remove_dir_all(&dir);
}

fn test_timeout_124_message() {
    let start = Instant::now();
    let o = tool_probe("caps", &["1"], "sleep", &["30000"]);
    let elapsed = start.elapsed();
    assert_eq!(o.status.code(), Some(124), "stderr={}", err(&o));
    assert_eq!(err(&o), TIMEOUT_1S);
    assert_eq!(out(&o), "");
    assert!(elapsed < Duration::from_secs(20), "elapsed={elapsed:?}");
}

fn test_cpu_cap_flags_visible() {
    let o = tool_probe("wnice", &["-c", "50", "--"], "jobinfo", &[]);
    assert_eq!(o.status.code(), Some(0), "stderr={}", err(&o));
    let report = last_line(&o);
    assert_ne!(
        job_field(&report, "limitflags") & JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        0,
        "KILL_ON_JOB_CLOSE must be set while the tree runs: {report}"
    );
    assert_eq!(
        job_field(&report, "cpuctrl"),
        JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
        "cpu rate control must be a hard cap: {report}"
    );
    assert_eq!(
        job_field(&report, "cpurate"),
        5000,
        "50% must map to CpuRate 5000: {report}"
    );
}

fn test_cpu_cap_survives_normal_exit() {
    // The other half of the lifecycle: after a NORMAL exit only the
    // kill-on-close guard is released, so a background child the command left
    // behind is still alive AND still inside the capped job. The report comes
    // from that survivor itself and is only published after the test saw wnice
    // exit; a cleanup that dropped the CPU cap would report cpurate=0 here.
    let scratch = ScratchDir::new("after");
    let report_path = scratch.join("job.txt");
    let go = scratch.join("go");
    let handle_path = scratch.join("handle");
    let script = format!(
        "(Start-Process -FilePath '{}' -WindowStyle Hidden -PassThru).Id",
        probe_exe().display()
    );
    check_probe_depth();
    let o = Command::new(tools_dir().join("wnice.exe"))
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
        .env(PROBE_JOBINFO_OUT, &report_path)
        .env(PROBE_JOBINFO_GO, &go)
        .env("WNICE_TEST_PID", std::process::id().to_string())
        .env("WNICE_TEST_HANDLE_OUT", &handle_path)
        .env("WNICE_PROBE_DEPTH", (probe_depth() + 1).to_string())
        .stdin(Stdio::null())
        .output()
        .expect("spawn wnice");
    // Adopt before asserting: the guard must be armed before anything can fail.
    let survivor = adopt_survivor(&handle_path);
    assert_eq!(o.status.code(), Some(0), "stderr={}", err(&o));
    assert!(survivor.alive(), "background child died with the wrapper");

    // wnice has exited (output() returned): only now release the survivor.
    std::fs::write(&go, b"").expect("release the survivor");
    let report = wait_for_job_report(&report_path, &survivor, Duration::from_secs(40));
    assert!(
        survivor.alive(),
        "the survivor exited before it could report"
    );
    assert_eq!(
        report.cpurate, 5000,
        "the CPU cap must still be on the job after the wrapper exited"
    );
    assert_eq!(
        report.cpuctrl,
        JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
        "cpu rate control must still be a hard cap after the wrapper exited"
    );
    assert_eq!(
        report.limitflags & JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        0,
        "KILL_ON_JOB_CLOSE must be released on a normal exit"
    );
    assert!(survivor.kill(), "cleanup kill failed for the survivor");
    // `scratch` drops here: no report and no "go" file survive this test.
}

fn test_daemon_survives_normal_exit() {
    // KILL_ON_JOB_CLOSE is released when the wrapped command exits normally, so
    // a background child the command leaves behind keeps its limits but is not
    // killed. The child is started through Start-Process: a directly spawned
    // detached process also dies when its parent exits with a piped stdout (see
    // tests/integration.rs), which would make it a false oracle.
    let scratch = ScratchDir::new("daemon");
    let handle_path = scratch.join("handle");
    let script = format!(
        "(Start-Process -FilePath '{}' -WindowStyle Hidden -PassThru).Id",
        probe_exe().display()
    );
    check_probe_depth();
    let o = Command::new(tools_dir().join("wnice.exe"))
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
        .env("WNICE_TEST_PID", std::process::id().to_string())
        .env("WNICE_TEST_HANDLE_OUT", &handle_path)
        .env("WNICE_PROBE_DEPTH", (probe_depth() + 1).to_string())
        .stdin(Stdio::null())
        .output()
        .expect("spawn wnice");
    // Adopt before asserting: the guard must be armed before anything can fail.
    let survivor = adopt_survivor(&handle_path);
    assert_eq!(o.status.code(), Some(0), "stderr={}", err(&o));
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(250));
        assert!(survivor.alive(), "background child died with the wrapper");
    }
    assert!(survivor.kill(), "cleanup kill failed for the survivor");
    let mut dead = false;
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(50));
        if !survivor.alive() {
            dead = true;
        }
    }
    assert!(dead, "child survived an explicit kill");
}

fn test_timeout_kills_whole_tree() {
    let start = Instant::now();
    let o = tool_probe("wnice", &["-s", "2", "--"], "spawnchain", &["15000"]);
    let elapsed = start.elapsed();
    assert_eq!(o.status.code(), Some(124), "stderr={}", err(&o));
    assert_eq!(err(&o), TIMEOUT_2S);
    assert!(elapsed < Duration::from_secs(15), "elapsed={elapsed:?}");

    let pids: Vec<u32> = last_line(&o)
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect();
    assert!(pids.len() >= 6, "spawnchain output: {}", out(&o));
    // Root, child and grandchild are the first three fields.
    std::thread::sleep(Duration::from_millis(300));
    for (generation, pid) in pids[..3].iter().enumerate() {
        assert!(
            !pid_alive(*pid),
            "generation {generation} survived the kill"
        );
    }
}

fn test_cy_cx_agent_flags() {
    let bin = std::env::temp_dir().join(format!("wnice-compat-agents-{}", std::process::id()));
    std::fs::create_dir_all(&bin).expect("agents dir");
    for (tool, agent, flag) in [
        ("cy", "claude", "--dangerously-skip-permissions"),
        ("cx", "codex", "--dangerously-bypass-approvals-and-sandbox"),
    ] {
        std::fs::copy(probe_exe(), bin.join(format!("{agent}.exe"))).expect("fake agent");
        // `--help` is forwarded like every other argument: cy/cx have no help
        // text of their own, so the flag must reach the agent verbatim.
        for (caller_args, want) in [
            (vec!["x y", "-p"], vec![flag, "x y", "-p"]),
            (vec!["--help"], vec![flag, "--help"]),
        ] {
            check_probe_depth();
            let o = Command::new(tools_dir().join(format!("{tool}.exe")))
                .creation_flags(CREATE_NO_WINDOW)
                .args(&caller_args)
                .env("PATH", &bin)
                .env("PATHEXT", ".COM;.EXE;.BAT;.CMD")
                .env("WNICE_PROBE", "echo")
                .env("WNICE_PROBE_ARGS", "")
                .env("WNICE_PROBE_DEPTH", (probe_depth() + 1).to_string())
                .stdin(Stdio::null())
                .output()
                .expect("spawn agent frontend");
            assert_eq!(
                o.status.code(),
                Some(0),
                "{tool} {caller_args:?}: {}",
                err(&o)
            );
            assert_eq!(last_line(&o), want.join("\u{1f}"), "{tool} {caller_args:?}");
        }
    }
    let _ = std::fs::remove_dir_all(&bin);
}

fn test_cy_cx_without_agent_127() {
    // No agent on PATH: the launcher reports the same command-not-found error
    // and exit code as any other unresolved bare name.
    let empty = std::env::temp_dir().join(format!("wnice-compat-noagent-{}", std::process::id()));
    std::fs::create_dir_all(&empty).expect("empty path dir");
    for (tool, agent) in [("cy", "claude"), ("cx", "codex")] {
        let want = format!(
            "wnice: '{agent}': command not found (searched PATH; the current directory is never searched)\n"
        );
        let o = Command::new(tools_dir().join(format!("{tool}.exe")))
            .creation_flags(CREATE_NO_WINDOW)
            .arg("--help")
            .env("PATH", &empty)
            .env("PATHEXT", ".COM;.EXE;.BAT;.CMD")
            .stdin(Stdio::null())
            .output()
            .expect("spawn agent frontend");
        assert_eq!(o.status.code(), Some(127), "{tool}: stderr={}", err(&o));
        assert_eq!(out(&o), "", "{tool}");
        assert_eq!(err(&o), want, "{tool}");
    }
    let _ = std::fs::remove_dir_all(&empty);
}

fn test_unknown_frontend() {
    // argv[0] that names no tool: main.rs falls through to its exit-2 error.
    let dir = std::env::temp_dir().join(format!("wnice-compat-unknown-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("unknown dir");
    let stem = "wnice-compat-not-a-tool";
    let exe = dir.join(format!("{stem}.exe"));
    std::fs::copy(env!("CARGO_BIN_EXE_wnice"), &exe).expect("copy exe");
    let o = Command::new(&exe)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .output()
        .expect("spawn unknown frontend");
    assert_eq!(o.status.code(), Some(2), "stderr={}", err(&o));
    assert_eq!(out(&o), "");
    assert_eq!(
        err(&o),
        format!("wnice: unknown frontend '{stem}'\nusage: wnice <options> <command> [args...]\n")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------ main ----

const TESTS: &[(&str, fn())] = &[
    ("help_goldens", test_help_goldens),
    ("help_flags", test_help_flags),
    ("version_shape", test_version_shape),
    ("wnice_usage_errors", test_wnice_usage_errors),
    ("legacy_usage_errors", test_legacy_usage_errors),
    ("all_options_accepted", test_all_options_accepted),
    ("exit_code_passthrough", test_exit_code_passthrough),
    ("command_not_found_127", test_command_not_found_127),
    (
        "percent_refusal_and_bat_exit_code",
        test_percent_refusal_and_bat_exit_code,
    ),
    ("timeout_124_message", test_timeout_124_message),
    ("cpu_cap_flags_visible", test_cpu_cap_flags_visible),
    (
        "cpu_cap_survives_normal_exit",
        test_cpu_cap_survives_normal_exit,
    ),
    (
        "daemon_survives_normal_exit",
        test_daemon_survives_normal_exit,
    ),
    ("timeout_kills_whole_tree", test_timeout_kills_whole_tree),
    ("cy_cx_agent_flags", test_cy_cx_agent_flags),
    ("cy_cx_without_agent_127", test_cy_cx_without_agent_127),
    ("unknown_frontend", test_unknown_frontend),
];

fn main() {
    // Daemon probe dispatch: this process is a survivor recording the job
    // limits that outlive wnice (see PROBE_JOBINFO_OUT).
    if let (Ok(out), Ok(go)) = (
        std::env::var(PROBE_JOBINFO_OUT),
        std::env::var(PROBE_JOBINFO_GO),
    ) {
        probe_jobinfo_after(&out, &go);
    }

    // Probe dispatch MUST come first: when WNICE_PROBE is set this process is a
    // probe child of wnice, not the test runner (see the module doc).
    if let Ok(mode) = std::env::var("WNICE_PROBE") {
        probe_main(&mode);
    }

    let mut failed: Vec<&str> = Vec::new();
    for (name, test) in TESTS {
        // Tests assert via panic; catch_unwind keeps one failure from aborting
        // the rest of the run.
        match std::panic::catch_unwind(test) {
            Ok(()) => println!("ok {name}"),
            Err(_) => {
                println!("FAILED {name}");
                failed.push(name);
            }
        }
    }
    let _ = std::fs::remove_dir_all(
        std::env::temp_dir().join(format!("wnice-compat-{}", std::process::id())),
    );
    let summary = if failed.is_empty() {
        String::new()
    } else {
        format!("; failed: {}", failed.join(", "))
    };
    println!(
        "{}/{} passed{}",
        TESTS.len() - failed.len(),
        TESTS.len(),
        summary
    );
    if !failed.is_empty() {
        std::process::exit(1);
    }
}
