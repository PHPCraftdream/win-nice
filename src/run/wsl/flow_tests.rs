#![cfg(test)]
//! Scripted-flow tests for the WSL engine: every precheck failure, the
//! happy path and the timeout decision, driven through `run_wsl_in` with a
//! fake wsl.exe driver — no WSL required, passes on plain Windows.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::time::Duration;

use super::exec::{run_wsl_in, ExecMode, ExecResult, WslExec};
use super::*;

// Duplicated from tests.rs: cfg(test) sibling modules cannot share private items.
const GIB: u64 = 1024 * 1024 * 1024;

// Duplicated from tests.rs: cfg(test) sibling modules cannot share private items.
fn vecs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

// Duplicated from tests.rs: cfg(test) sibling modules cannot share private items.
fn facts(vcpu: u32, mem_bytes: u64) -> GuestFacts {
    GuestFacts { vcpu, mem_bytes }
}

// Duplicated from tests.rs: cfg(test) sibling modules cannot share private items.
fn distro_list_text() -> String {
    "  NAME                   STATE           VERSION\r\n\
     * Ubuntu-24.04           Running         2\r\n\
       Debian-12              Stopped         2\r\n"
        .to_string()
}

/// One queued `WslExec` outcome (all-ASCII payloads: the decoder treats
/// them as plain UTF-8, exactly like probe output from Linux commands).
fn ok(code: i32, stdout: &str, stderr: &str) -> Result<ExecResult, String> {
    Ok(ExecResult {
        code,
        stdout: stdout.as_bytes().to_vec(),
        stderr: stderr.as_bytes().to_vec(),
    })
}

#[derive(Debug, Clone)]
struct RecordedCall {
    wsl_exe: String,
    args: Vec<String>,
    mode: ExecMode,
    timeout: Option<Duration>,
}

/// Scripted fake: pops queued results in order (panics on an unscripted
/// extra call, so a flow change that adds a probe fails loudly here) and
/// records every call for assertions.
struct FakeWslExec {
    script: RefCell<VecDeque<Result<ExecResult, String>>>,
    calls: RefCell<Vec<RecordedCall>>,
    sleep_ms: u64,
}

impl FakeWslExec {
    fn new() -> Self {
        FakeWslExec {
            script: RefCell::new(VecDeque::new()),
            calls: RefCell::new(Vec::new()),
            sleep_ms: 0,
        }
    }

    fn push(self, r: Result<ExecResult, String>) -> Self {
        self.script.borrow_mut().push_back(r);
        self
    }

    fn with_sleep_ms(mut self, ms: u64) -> Self {
        self.sleep_ms = ms;
        self
    }
}

impl WslExec for FakeWslExec {
    fn run(
        &self,
        wsl_exe: &str,
        args: &[String],
        mode: ExecMode,
        timeout: Option<Duration>,
    ) -> Result<ExecResult, String> {
        self.calls.borrow_mut().push(RecordedCall {
            wsl_exe: wsl_exe.to_string(),
            args: args.to_vec(),
            mode,
            timeout,
        });
        if self.sleep_ms > 0 {
            std::thread::sleep(Duration::from_millis(self.sleep_ms));
        }
        self.script
            .borrow_mut()
            .pop_front()
            .expect("FakeWslExec: more calls than scripted")
    }
}

/// Script the seven precheck probes that always run, in flow order: distro
/// list, PID 1, cgroup v2, systemd-run, default user, nice/runuser chain,
/// cwd test. Fact probes and the final Inherit run are pushed by the test.
fn happy() -> FakeWslExec {
    FakeWslExec::new()
        .push(ok(0, &distro_list_text(), ""))
        .push(ok(0, "systemd\n", ""))
        .push(ok(0, "cpuset cpu io memory pids\n", ""))
        .push(ok(0, "systemd 252\n", ""))
        .push(ok(0, "me\n", ""))
        .push(ok(0, "", ""))
        .push(ok(0, "", ""))
}

fn all_flags_cfg() -> Config {
    Config {
        wsl: Some(WslMode {
            distro: Some("Ubuntu-24.04".to_string()),
        }),
        priority: Some(PriorityClass::Idle),
        cpu_percent: Some(50),
        memory: Some(MemSpec::Bytes(2 * GIB)),
        threads: Some(4),
        procs: Some(64),
        timeout_ms: Some(2500),
        boost: false,
        no_servers: false,
        adopt: Vec::new(),
        command: vecs(&["sleep", "1"]),
    }
}

/// Drive the flow with a Windows cwd of `C:\work` (-> `/mnt/c/work`).
fn flow(cfg: &Config, exec: &FakeWslExec) -> (i32, Vec<String>) {
    let mut msgs = Vec::new();
    let code = run_wsl_in(
        cfg,
        exec,
        r"C:\Windows\System32\wsl.exe",
        r"C:\work",
        &mut |m| msgs.push(m.to_string()),
    );
    (code, msgs)
}

fn assert_flow_fails(exec: FakeWslExec, cfg: &Config, want: &str) {
    let (code, msgs) = flow(cfg, &exec);
    assert_eq!(code, 1);
    assert_eq!(msgs, vec![want.to_string()]);
}

// ---- flow with FakeWslExec ----

#[test]
fn flow_no_command_fails_before_any_call() {
    let exec = FakeWslExec::new();
    let cfg = Config {
        wsl: Some(WslMode::default()),
        ..Default::default()
    };
    let (code, msgs) = flow(&cfg, &exec);
    assert_eq!(code, 1);
    assert_eq!(msgs, vec!["wnice: --wsl: no command given".to_string()]);
    assert!(exec.calls.borrow().is_empty());
}

#[test]
fn flow_unc_cwd_fails_before_any_call() {
    let exec = FakeWslExec::new();
    let mut msgs = Vec::new();
    let code = run_wsl_in(
        &Config {
            wsl: Some(WslMode::default()),
            command: vecs(&["true"]),
            ..Default::default()
        },
        &exec,
        "wsl.exe",
        r"\\server\share\dir",
        &mut |m| msgs.push(m.to_string()),
    );
    assert_eq!(code, 1);
    assert_eq!(
        msgs,
        vec!["wnice: --wsl: cannot translate the current directory (UNC paths are not supported in WSL mode): \\\\server\\share\\dir".to_string()]
    );
    assert!(exec.calls.borrow().is_empty());
}

#[test]
fn flow_wsl_exe_transport_error() {
    let exec = FakeWslExec::new().push(Err("program not found".to_string()));
    let cfg = Config {
        wsl: Some(WslMode::default()),
        command: vecs(&["true"]),
        ..Default::default()
    };
    assert_flow_fails(
        exec,
        &cfg,
        "wnice: --wsl: failed to query WSL distributions: program not found",
    );
}

#[test]
fn flow_distro_list_nonzero_exit() {
    let exec = FakeWslExec::new().push(ok(1, "", "WslRegisterDistribution failed"));
    let cfg = Config {
        wsl: Some(WslMode::default()),
        command: vecs(&["true"]),
        ..Default::default()
    };
    assert_flow_fails(
        exec,
        &cfg,
        "wnice: --wsl: failed to query WSL distributions: WslRegisterDistribution failed",
    );
}

#[test]
fn flow_distro_not_found() {
    let exec = happy();
    let cfg = Config {
        wsl: Some(WslMode {
            distro: Some("Alpine".to_string()),
        }),
        command: vecs(&["true"]),
        ..Default::default()
    };
    assert_flow_fails(
        exec,
        &cfg,
        "wnice: --wsl: WSL distribution 'Alpine' not found (available: Ubuntu-24.04, Debian-12)",
    );
}

#[test]
fn flow_distro_is_wsl1() {
    let exec = FakeWslExec::new().push(ok(0, "* Solo-24.04           Running         1\r\n", ""));
    let cfg = Config {
        wsl: Some(WslMode::default()),
        command: vecs(&["true"]),
        ..Default::default()
    };
    assert_flow_fails(
        exec,
        &cfg,
        "wnice: --wsl: WSL distribution 'Solo-24.04' is WSL1, not WSL2",
    );
}

#[test]
fn flow_pid1_not_systemd() {
    let exec = FakeWslExec::new()
        .push(ok(0, &distro_list_text(), ""))
        .push(ok(0, "init\n", ""));
    let cfg = Config {
        wsl: Some(WslMode::default()),
        command: vecs(&["true"]),
        ..Default::default()
    };
    assert_flow_fails(
        exec,
        &cfg,
        "wnice: --wsl: WSL distribution 'Ubuntu-24.04' does not run systemd as PID 1 (got 'init'); set [boot] systemd=true in /etc/wsl.conf and restart WSL",
    );
}

#[test]
fn flow_pid1_probe_transport_error_reports_unknown() {
    let exec = FakeWslExec::new()
        .push(ok(0, &distro_list_text(), ""))
        .push(Err("cannot start".to_string()));
    let cfg = Config {
        wsl: Some(WslMode::default()),
        command: vecs(&["true"]),
        ..Default::default()
    };
    assert_flow_fails(
        exec,
        &cfg,
        "wnice: --wsl: WSL distribution 'Ubuntu-24.04' does not run systemd as PID 1 (got '(unknown)'); set [boot] systemd=true in /etc/wsl.conf and restart WSL",
    );
}

#[test]
fn flow_cgroup_v2_missing() {
    let exec = FakeWslExec::new()
        .push(ok(0, &distro_list_text(), ""))
        .push(ok(0, "systemd\n", ""))
        .push(ok(1, "", ""));
    let cfg = Config {
        wsl: Some(WslMode::default()),
        command: vecs(&["true"]),
        ..Default::default()
    };
    assert_flow_fails(
        exec,
        &cfg,
        "wnice: --wsl: cgroup v2 is not available in 'Ubuntu-24.04'",
    );
}

#[test]
fn flow_systemd_run_missing() {
    let exec = FakeWslExec::new()
        .push(ok(0, &distro_list_text(), ""))
        .push(ok(0, "systemd\n", ""))
        .push(ok(0, "cpuset cpu io memory pids\n", ""))
        .push(ok(127, "", ""));
    let cfg = Config {
        wsl: Some(WslMode::default()),
        command: vecs(&["true"]),
        ..Default::default()
    };
    assert_flow_fails(
        exec,
        &cfg,
        "wnice: --wsl: systemd-run is not available in 'Ubuntu-24.04' (is systemd fully installed?)",
    );
}

#[test]
fn flow_user_probe_fails() {
    let exec = FakeWslExec::new()
        .push(ok(0, &distro_list_text(), ""))
        .push(ok(0, "systemd\n", ""))
        .push(ok(0, "cpuset cpu io memory pids\n", ""))
        .push(ok(0, "systemd 252\n", ""))
        .push(ok(1, "", "whoami: cannot find name"));
    let cfg = Config {
        wsl: Some(WslMode::default()),
        command: vecs(&["true"]),
        ..Default::default()
    };
    assert_flow_fails(
        exec,
        &cfg,
        "wnice: --wsl: failed to determine the default guest user of 'Ubuntu-24.04': whoami: cannot find name",
    );
}

#[test]
fn flow_runuser_chain_probe_fails() {
    let exec = FakeWslExec::new()
        .push(ok(0, &distro_list_text(), ""))
        .push(ok(0, "systemd\n", ""))
        .push(ok(0, "cpuset cpu io memory pids\n", ""))
        .push(ok(0, "systemd 252\n", ""))
        .push(ok(0, "me\n", ""))
        .push(ok(1, "", "runuser: Authentication failure"));
    let cfg = Config {
        wsl: Some(WslMode::default()),
        command: vecs(&["true"]),
        ..Default::default()
    };
    assert_flow_fails(
        exec,
        &cfg,
        "wnice: --wsl: 'nice runuser -u me' failed inside 'Ubuntu-24.04' (exit 1): runuser: Authentication failure",
    );
}

#[test]
fn flow_cwd_missing_in_guest() {
    let exec = FakeWslExec::new()
        .push(ok(0, &distro_list_text(), ""))
        .push(ok(0, "systemd\n", ""))
        .push(ok(0, "cpuset cpu io memory pids\n", ""))
        .push(ok(0, "systemd 252\n", ""))
        .push(ok(0, "me\n", ""))
        .push(ok(0, "", ""))
        .push(ok(1, "", ""));
    let cfg = Config {
        wsl: Some(WslMode::default()),
        command: vecs(&["true"]),
        ..Default::default()
    };
    assert_flow_fails(
        exec,
        &cfg,
        "wnice: --wsl: the current Windows directory is not visible in the guest (/mnt/c/work missing - is the drive mounted?)",
    );
}

#[test]
fn flow_nproc_probe_fails_when_cpu_percent_given() {
    let exec = happy().push(ok(0, "", ""));
    let cfg = Config {
        wsl: Some(WslMode::default()),
        cpu_percent: Some(50),
        command: vecs(&["true"]),
        ..Default::default()
    };
    assert_flow_fails(
        exec,
        &cfg,
        "wnice: --wsl: failed to determine the guest vCPU count",
    );
}

#[test]
fn flow_meminfo_probe_fails_when_percent_memory_given() {
    let exec = happy().push(ok(0, "MemFree: 1 kB\n", ""));
    let cfg = Config {
        wsl: Some(WslMode::default()),
        memory: Some(MemSpec::Percent(50)),
        command: vecs(&["true"]),
        ..Default::default()
    };
    assert_flow_fails(
        exec,
        &cfg,
        "wnice: --wsl: failed to determine the guest memory size",
    );
}

#[test]
fn flow_compose_refusal_realtime() {
    // All probes succeed; compose refuses -p realtime.
    let exec = happy();
    let cfg = Config {
        wsl: Some(WslMode::default()),
        priority: Some(PriorityClass::Realtime),
        command: vecs(&["true"]),
        ..Default::default()
    };
    assert_flow_fails(exec, &cfg, REALTIME_REFUSAL);
}

#[test]
fn flow_happy_path_all_flags_single_inherit_call() {
    let exec = happy()
        .push(ok(0, "16\n", "")) // nproc --all (cpu_percent + threads)
        .push(ok(7, "", "")); // final Inherit run, exit code 7
    let cfg = all_flags_cfg();
    let (code, msgs) = flow(&cfg, &exec);
    assert_eq!(code, 7);
    assert!(msgs.is_empty());

    let calls = exec.calls.borrow();
    assert_eq!(calls.len(), 9);
    // Every probe: captured, 120 s cap, wsl.exe path passed through.
    for c in &calls[..8] {
        assert_eq!(c.wsl_exe, r"C:\Windows\System32\wsl.exe");
        assert_eq!(c.mode, ExecMode::Capture);
        assert_eq!(c.timeout, Some(Duration::from_secs(120)));
    }
    assert_eq!(calls[0].args, vecs(&["--list", "--verbose"]));
    // Exactly ONE Inherit call, unbounded, and its argv equals the compose
    // golden for the unit it actually carried.
    let last = calls.last().unwrap();
    assert_eq!(last.mode, ExecMode::Inherit);
    assert_eq!(last.timeout, None);
    let unit_el = last
        .args
        .iter()
        .find(|a| a.starts_with("--unit="))
        .expect("--unit element");
    let unit = unit_el.trim_start_matches("--unit=").to_string();
    assert!(unit.starts_with("wnice-"), "{unit}");
    let facts = facts(16, 0);
    let golden = compose(&cfg, &facts, "me", "/mnt/c/work", &unit).unwrap();
    assert_eq!(last.args, golden.wsl_args);
}

#[test]
fn flow_default_distro_final_run_has_no_dash_d() {
    let exec = happy().push(ok(0, "", ""));
    let cfg = Config {
        wsl: Some(WslMode::default()),
        command: vecs(&["true"]),
        ..Default::default()
    };
    let (code, msgs) = flow(&cfg, &exec);
    assert_eq!(code, 0);
    assert!(msgs.is_empty());
    let calls = exec.calls.borrow();
    assert_eq!(calls.len(), 8); // 7 probes + final run
                                // The six guest probes legitimately target the RESOLVED default distro
                                // explicitly (-d); only the FINAL launch must omit -d: with no distro
                                // named, wsl.exe applies its own default.
    assert!(calls[1..7]
        .iter()
        .all(|c| c.args.contains(&"-d".to_string())));
    assert!(!calls.last().unwrap().args.contains(&"-d".to_string()));
}

#[test]
fn flow_timeout_returns_124_with_message() {
    // Sleep on every call so the final run's elapsed time exceeds the 1 ms
    // timeout; the scope kill itself is simulated by the Inherit code 0.
    let exec = happy()
        .push(ok(0, "16\n", ""))
        .push(ok(0, "", ""))
        .with_sleep_ms(5);
    let mut cfg = all_flags_cfg();
    cfg.timeout_ms = Some(1);
    let (code, msgs) = flow(&cfg, &exec);
    assert_eq!(code, 124);
    assert_eq!(
        msgs,
        vec!["wnice: timed out after 0.001s - the systemd scope was killed inside WSL".to_string()]
    );
}

#[test]
fn flow_elapsed_below_timeout_keeps_exit_code() {
    let exec = happy().push(ok(0, "16\n", "")).push(ok(7, "", ""));
    let cfg = all_flags_cfg(); // timeout 2500 ms, no sleeping
    let (code, msgs) = flow(&cfg, &exec);
    assert_eq!(code, 7);
    assert!(msgs.is_empty());
}
