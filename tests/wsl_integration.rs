//! Real-WSL integration tests: each test launches the wrapped command inside
//! an actual WSL2 distro through the product code path
//! (`run::run` -> `run::wsl`), asserting the systemd-scope flow end to end.
//!
//! tests/wsl_integration.rs launches a real distro and is skipped unless
//! WNICE_TEST_WSL=1. Set WNICE_TEST_WSL_DISTRO=<name> to pin a distro;
//! otherwise wnice uses wsl.exe's default. Run with
//! `WNICE_TEST_WSL=1 cargo test --test wsl_integration`. The gate defaults to
//! skipped so plain `cargo test` and CI (windows-latest, no systemd distro
//! guaranteed) stay green.
//!
//! Standard test harness (`#[test]` fns); the test only builds `Config`s and
//! calls the library — nothing is spawned directly, so every process goes
//! through the product code, which sets CREATE_NO_WINDOW. No windows, no
//! prompts, std-only.

use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use wnice::run::wsl::{translate_windows_cwd, WslMode};
use wnice::{run, Config, MemSpec, PriorityClass, RealSys};

/// The gate: tests run only when WNICE_TEST_WSL is set to something other
/// than "0". `cargo test` without the variable compiles the tests but skips
/// every body.
fn wsl_enabled() -> bool {
    matches!(env::var("WNICE_TEST_WSL").as_deref(), Ok(v) if v != "0")
}

/// Distro override: WNICE_TEST_WSL_DISTRO (non-empty) pins the distro;
/// empty/unset means wsl.exe's default.
fn distro_override() -> Option<String> {
    env::var("WNICE_TEST_WSL_DISTRO")
        .ok()
        .filter(|d| !d.is_empty())
}

/// Base builder: WSL mode + the guest command, distro per
/// `WNICE_TEST_WSL_DISTRO`. Every test goes through a builder so the distro
/// contract stays in one place.
fn wsl_cfg(command: Vec<String>) -> Config {
    Config {
        wsl: Some(WslMode {
            distro: distro_override(),
        }),
        command,
        ..Default::default()
    }
}

/// Builder with the distro forced (used by the failure-path tests, which
/// must not depend on the operator's override variable).
fn wsl_cfg_with(distro: Option<&str>, command: Vec<String>) -> Config {
    Config {
        wsl: Some(WslMode {
            distro: distro.map(str::to_string),
        }),
        command,
        ..Default::default()
    }
}

/// Unique per-pid temp file path on the Windows side; `suffix` keeps parallel
/// tests from colliding.
fn temp_file(suffix: &str) -> PathBuf {
    env::temp_dir().join(format!("wnice-wsl-{suffix}-{}.txt", std::process::id()))
}

/// Best-effort cleanup in all paths (assert failures included).
fn cleanup(path: &PathBuf) {
    let _ = std::fs::remove_file(path);
}

/// The gate is checked first in every test body below.
macro_rules! gated {
    () => {
        if !wsl_enabled() {
            eprintln!("skipping: WNICE_TEST_WSL is not set");
            return;
        }
    };
}

#[test]
fn wsl_true_exits_0() {
    gated!();
    let cfg = wsl_cfg(vec!["/bin/true".to_string()]);
    assert_eq!(run::run(&cfg, Arc::new(RealSys)), 0);
}

#[test]
fn wsl_exit_code_propagates() {
    gated!();
    // /bin/sh here is the TEST's own choice for producing a known exit
    // code; the product never shells — guest argv stays separate elements.
    let cfg = wsl_cfg(vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        "exit 7".to_string(),
    ]);
    assert_eq!(run::run(&cfg, Arc::new(RealSys)), 7);
}

#[test]
fn wsl_arguments_reach_the_guest_verbatim() {
    gated!();
    // THE argv-integrity test: the sh script is fixed; everything after it
    // is DATA that must arrive byte-for-byte. Metacharacters, spaces, an
    // empty string and non-ASCII all ride the same argv. If this fails ONLY
    // on the empty element, that is the known wsl.exe relay caveat (the
    // relay can drop empty-string arguments) — report it, do not "fix" the
    // product.
    let out = temp_file("args");
    let out_str = out.to_str().expect("temp path is valid UTF-8");
    let guest_path = translate_windows_cwd(out_str).expect("temp dir translates to a guest path");
    let cfg = wsl_cfg(vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        format!("printf '%s\\n' \"$@\" > {guest_path}"),
        "sh".to_string(),
        "a;b".to_string(),
        "c&d".to_string(),
        "e|f".to_string(),
        "$HOME".to_string(),
        "$(id)".to_string(),
        "back`tick".to_string(),
        "q\"q".to_string(),
        "with space".to_string(),
        "100%".to_string(),
        "".to_string(),
        "Ø—日本".to_string(),
        "-n".to_string(),
    ]);
    let code = run::run(&cfg, Arc::new(RealSys));
    let contents = std::fs::read_to_string(&out).unwrap_or_default();
    cleanup(&out);
    assert_eq!(code, 0);
    // printf '%s\n' appends a trailing newline after the last argument;
    // split drops the final empty element, so the empty-string argument
    // itself still shows up as an empty line in the middle... except as the
    // 13th of 13 args it is the LAST line before the trailing "" from split.
    let mut lines: Vec<&str> = contents.split('\n').collect();
    assert_eq!(lines.pop(), Some(""), "trailing newline from printf");
    let expected = vec![
        "a;b",
        "c&d",
        "e|f",
        "$HOME",
        "$(id)",
        "back`tick",
        "q\"q",
        "with space",
        "100%",
        "",
        "Ø—日本",
        "-n",
    ];
    assert_eq!(lines, expected);
}

#[test]
fn wsl_working_directory_is_translated() {
    gated!();
    let out = temp_file("pwd");
    let out_str = out.to_str().expect("temp path is valid UTF-8");
    let guest_path = translate_windows_cwd(out_str).expect("temp dir translates to a guest path");
    let cfg = wsl_cfg(vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        format!("pwd > {guest_path}"),
    ]);
    let code = run::run(&cfg, Arc::new(RealSys));
    let contents = std::fs::read_to_string(&out).unwrap_or_default();
    cleanup(&out);
    assert_eq!(code, 0);
    let win_cwd = env::current_dir().expect("current dir");
    let expected = translate_windows_cwd(win_cwd.to_str().expect("cwd is valid UTF-8"))
        .expect("cwd translates");
    // The guest cwd must equal the translated Windows cwd (`--cd` honored).
    assert_eq!(contents.trim(), expected);
}

#[test]
fn wsl_timeout_returns_124_quickly() {
    gated!();
    // RuntimeMaxSec + KillSignal=SIGKILL kill the scope INSIDE the guest at
    // the deadline; the Windows side then maps "deadline reached" to exit
    // 124. sleep 30 against a 2 s timeout: if the guest kill never happened,
    // the run would block the full 30 s (wnice's Inherit wait has no Windows
    // timeout), so asserting elapsed < 30 proves the kill was real.
    let mut cfg = wsl_cfg(vec!["/bin/sleep".to_string(), "30".to_string()]);
    cfg.timeout_ms = Some(2000);
    let started = Instant::now();
    let code = run::run(&cfg, Arc::new(RealSys));
    let elapsed = started.elapsed();
    assert_eq!(code, 124);
    assert!(
        elapsed.as_secs() < 30,
        "timeout kill did not fire: ran {elapsed:?}"
    );
}

#[test]
fn wsl_systemd_accepts_every_mapped_property() {
    gated!();
    // THE mapping-validation test: systemd-run validates every -p property
    // when it creates the unit — an invalid or unknown property fails the
    // run before the command starts. Exit 0 therefore proves the composed
    // property set (CPUQuota, CPUWeight, MemoryMax, AllowedCPUs, TasksMax,
    // RuntimeMaxSec, KillSignal) is accepted by the real systemd. /bin/true
    // keeps it fast; property acceptance does not need runtime.
    let cfg = Config {
        priority: Some(PriorityClass::Idle),
        cpu_percent: Some(1),
        memory: Some(MemSpec::Bytes(256 * 1024 * 1024)),
        threads: Some(1),
        procs: Some(50),
        timeout_ms: Some(60_000),
        boost: false,
        audit: false,
        no_servers: false,
        wsl: Some(WslMode {
            distro: distro_override(),
        }),
        command: vec!["/bin/true".to_string()],
    };
    assert_eq!(run::run(&cfg, Arc::new(RealSys)), 0);
}

#[test]
fn wsl_unknown_distro_fails_fast() {
    gated!();
    // Forced distro name regardless of WNICE_TEST_WSL_DISTRO: the precheck
    // must refuse before touching the guest.
    let cfg = wsl_cfg_with(
        Some("wnice-no-such-distro-xyz"),
        vec!["/bin/true".to_string()],
    );
    assert_eq!(run::run(&cfg, Arc::new(RealSys)), 1);
}

#[test]
fn wsl_realtime_refused() {
    gated!();
    // Realtime runs the full probe sequence on real WSL, then is refused at
    // compose (the Linux nice/cgroup-weight model has no realtime band).
    let cfg = wsl_cfg_with(distro_override().as_deref(), vec!["/bin/true".to_string()]);
    let cfg = Config {
        priority: Some(PriorityClass::Realtime),
        ..cfg
    };
    assert_eq!(run::run(&cfg, Arc::new(RealSys)), 1);
}
