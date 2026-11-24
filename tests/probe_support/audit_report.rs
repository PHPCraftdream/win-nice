//! `wnice --audit` and `WNICE_AUDIT=1`, end to end (harness = false): the
//! report appears only when asked for and never changes the command's exit
//! code. Probes are THIS binary re-executed with WNICE_PROBE set
//! (tests/probe_support/). The file lives next to the probe module because
//! tests/ is at its entry limit (scripts/lint/lint-structure.js).

#![cfg(windows)]

#[allow(dead_code)] // the probe module carries modes this suite does not use
#[path = "mod.rs"]
mod probe_support;
use probe_support::{check_probe_depth, probe_depth, probe_exe, CREATE_NO_WINDOW};
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Output};

struct ChildGuard(Option<Child>);

impl ChildGuard {
    fn spawn(mut command: Command) -> Self {
        Self(Some(command.spawn().expect("spawn child")))
    }

    fn output(mut self) -> Output {
        self.0
            .take()
            .expect("child")
            .wait_with_output()
            .expect("wait for child")
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct RemoveDir(std::path::PathBuf);

impl Drop for RemoveDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn probe(command: &str, exit: &str) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wnice"));
    cmd.creation_flags(CREATE_NO_WINDOW);
    if command == "audit" {
        cmd.arg("--audit");
    }
    cmd.args(["--", probe_exe().to_str().unwrap(), "exit", exit])
        .env_remove("WNICE_AUDIT")
        .env("WNICE_PROBE", "exit")
        .env("WNICE_PROBE_ARGS", exit)
        .env("WNICE_PROBE_DEPTH", (probe_depth() + 1).to_string());
    ChildGuard::spawn(cmd).output()
}

fn audit_opt_in_and_defaults() {
    let plain = probe("plain", "0");
    assert_eq!(plain.status.code(), Some(0), "{}", stderr(&plain));
    assert!(stderr(&plain).is_empty(), "{}", stderr(&plain));

    let o = probe("audit", "0");
    let report = stderr(&o);
    assert_eq!(o.status.code(), Some(0), "{report}");
    for key in [
        "total=",
        "active=",
        "terminated=",
        "user=",
        "kernel=",
        "peak-job-memory=",
    ] {
        assert!(report.contains(key), "missing {key}: {report}");
    }
    assert!(report.contains("suspects: none"), "{report}");
    assert!(report.contains("wnice audit limitations:"), "{report}");

    let nonzero = probe("audit", "7");
    assert_eq!(nonzero.status.code(), Some(7), "{}", stderr(&nonzero));
    assert!(nonzero.stdout.is_empty());
}

fn legacy_capc_audit_env() {
    check_probe_depth();
    let dir = std::env::temp_dir().join(format!("wnice-audit-capc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create legacy temp directory");
    let _remove = RemoveDir(dir.clone());
    let exe = dir.join("capc.exe");
    std::fs::copy(env!("CARGO_BIN_EXE_wnice"), &exe).expect("copy wnice.exe");
    let mut cmd = Command::new(exe);
    cmd.creation_flags(CREATE_NO_WINDOW)
        .args(["50", probe_exe().to_str().unwrap(), "exit", "0"])
        .env("WNICE_PROBE", "exit")
        .env("WNICE_PROBE_ARGS", "0")
        .env("WNICE_PROBE_DEPTH", (probe_depth() + 1).to_string())
        .env("WNICE_AUDIT", "1");
    let o = ChildGuard::spawn(cmd).output();
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(stderr(&o).contains("wnice audit:"), "{}", stderr(&o));
    assert!(o.stdout.is_empty());
}

fn legacy_wnice_env_audit() {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wnice"));
    cmd.creation_flags(CREATE_NO_WINDOW)
        .args(["--", probe_exe().to_str().unwrap(), "exit", "7"])
        .env("WNICE_AUDIT", "1")
        .env("WNICE_PROBE", "exit")
        .env("WNICE_PROBE_ARGS", "7")
        .env("WNICE_PROBE_DEPTH", (probe_depth() + 1).to_string());
    let o = ChildGuard::spawn(cmd).output();
    assert_eq!(o.status.code(), Some(7), "{}", stderr(&o));
    assert!(stderr(&o).contains("wnice audit:"), "{}", stderr(&o));
    assert!(o.stdout.is_empty());
}

fn main() {
    if let Ok(mode) = std::env::var("WNICE_PROBE") {
        probe_support::probe_main(&mode);
    }
    audit_opt_in_and_defaults();
    println!("ok audit_opt_in_and_defaults");
    legacy_capc_audit_env();
    println!("ok legacy_capc_audit_env");
    legacy_wnice_env_audit();
    println!("ok legacy_wnice_env_audit");
}
