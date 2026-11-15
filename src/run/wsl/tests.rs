#![cfg(test)]
//! Pure-logic, compose-golden, metachar-safety and scripted-flow tests for
//! WSL mode. No WSL is required: the flow runs against `FakeWslExec`, so
//! the whole suite passes on plain Windows.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::time::Duration;

use super::exec::{run_wsl_in, ExecMode, ExecResult, WslExec};
use super::*;

const GIB: u64 = 1024 * 1024 * 1024;

fn vecs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// GuestFacts literal helper (`GuestFacts` deliberately has no Default: an
/// all-zero value means "nothing probed", never write it by accident).
fn facts(vcpu: u32, mem_bytes: u64) -> GuestFacts {
    GuestFacts { vcpu, mem_bytes }
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

/// Real-looking decoded `wsl -l -v` output: one starred default distro.
fn distro_list_text() -> String {
    "  NAME                   STATE           VERSION\r\n\
     * Ubuntu-24.04           Running         2\r\n\
       Debian-12              Stopped         2\r\n"
        .to_string()
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

// ---- translate_windows_cwd ----

#[test]
fn translate_cwd_drive_paths() {
    assert_eq!(translate_windows_cwd(r"C:\a\b").unwrap(), "/mnt/c/a/b");
    assert_eq!(translate_windows_cwd(r"D:\").unwrap(), "/mnt/d");
    assert_eq!(
        translate_windows_cwd(r"Z:\Weird Dir\Ø").unwrap(),
        "/mnt/z/Weird Dir/Ø"
    );
    assert_eq!(translate_windows_cwd(r"\\?\C:\x").unwrap(), "/mnt/c/x");
    // Lowercase drive in, lowercase out.
    assert_eq!(
        translate_windows_cwd(r"c:\lower\drive").unwrap(),
        "/mnt/c/lower/drive"
    );
    assert_eq!(
        translate_windows_cwd("C:/fwd/slash").unwrap(),
        "/mnt/c/fwd/slash"
    );
}

#[test]
fn translate_cwd_refusals() {
    let unc = translate_windows_cwd(r"\\server\share\dir").unwrap_err();
    assert!(unc.contains("UNC paths are not supported"), "{unc}");
    let unc_prefix = translate_windows_cwd(r"\\?\UNC\server\share").unwrap_err();
    assert!(
        unc_prefix.contains("UNC paths are not supported"),
        "{unc_prefix}"
    );
    assert_eq!(
        translate_windows_cwd(r"relative\path").unwrap_err(),
        "wnice: --wsl: current directory is not a Windows drive path: relative\\path"
    );
    // Drive letter without a separator, empty, and non-drive first char.
    assert!(translate_windows_cwd("C:").is_err());
    assert!(translate_windows_cwd("").is_err());
    assert!(translate_windows_cwd(r"1:\x").is_err());
}

// ---- unit_name ----

#[test]
fn unit_name_format_and_distinct() {
    assert_eq!(unit_name(42, 1_731_700_000_123), "wnice-42-1731700000123");
    assert_ne!(unit_name(1, 2), unit_name(2, 1));
}

// ---- decode_wsl_output ----

#[test]
fn decode_utf16le_with_bom() {
    let text = "NAME STATE VERSION\r\n* Ubuntu-24.04    Running         2\r\n";
    let mut units = vec![0xFEFFu16];
    units.extend(text.encode_utf16());
    let mut bytes = Vec::new();
    for u in units {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    assert_eq!(decode_wsl_output(&bytes), text);
}

#[test]
fn decode_utf16le_without_bom_detected_by_nul_density() {
    let text = "systemd 252";
    let mut bytes = Vec::new();
    for u in text.encode_utf16() {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    // ASCII text in UTF-16LE is half NUL bytes (>25%) and even-length.
    assert_eq!(decode_wsl_output(&bytes), text);
}

#[test]
fn decode_utf8_passthrough() {
    assert_eq!(decode_wsl_output(b"systemd\n"), "systemd\n");
    // Even-length UTF-8 without a NUL-heavy profile must stay UTF-8.
    assert_eq!(decode_wsl_output("Ø—日本".as_bytes()), "Ø—日本");
    assert_eq!(decode_wsl_output(b"ab"), "ab");
}

// ---- parse_distro_list ----

#[test]
fn parse_distro_list_preserves_names_and_star() {
    let text = "  NAME                   STATE           VERSION\r\n\
     * Ubuntu-24.04           Running         2\r\n\
       Ubuntu 22.04 LTS        Running         2\r\n\
       Debian-12               Stopped         1\r\n\
     garbage-x\r\n\
     \r\n";
    let list = parse_distro_list(text);
    assert_eq!(
        list,
        vec![
            DistroEntry {
                name: "Ubuntu-24.04".to_string(),
                state: "Running".to_string(),
                version: "2".to_string(),
                is_default: true,
            },
            DistroEntry {
                name: "Ubuntu 22.04 LTS".to_string(),
                state: "Running".to_string(),
                version: "2".to_string(),
                is_default: false,
            },
            DistroEntry {
                name: "Debian-12".to_string(),
                state: "Stopped".to_string(),
                version: "1".to_string(),
                is_default: false,
            },
        ]
    );
}

// ---- pick_distro ----

#[test]
fn pick_distro_default_exact_and_case_insensitive() {
    let list = parse_distro_list(&distro_list_text());
    assert_eq!(pick_distro(&list, None).unwrap().name, "Ubuntu-24.04");
    assert_eq!(
        pick_distro(&list, Some("Debian-12")).unwrap().name,
        "Debian-12"
    );
    assert_eq!(
        pick_distro(&list, Some("UBUNTU-24.04")).unwrap().name,
        "Ubuntu-24.04"
    );
    let err = pick_distro(&list, Some("Alpine")).unwrap_err();
    assert_eq!(
        err,
        "wnice: --wsl: WSL distribution 'Alpine' not found (available: Ubuntu-24.04, Debian-12)"
    );
}

#[test]
fn pick_distro_no_default_refusal() {
    let list = parse_distro_list("  Debian-12   Stopped   2\r\n");
    assert_eq!(
        pick_distro(&list, None).unwrap_err(),
        "wnice: --wsl: no default WSL distribution found (installed: none)"
    );
    assert_eq!(
        pick_distro(&[], None).unwrap_err(),
        "wnice: --wsl: no default WSL distribution found (installed: none)"
    );
}

// ---- parse_meminfo_kb / parse_nproc ----

#[test]
fn parse_meminfo_kb_finds_total() {
    let text =
        "MemTotal:       32695656 kB\nMemFree:         1234567 kB\nHugePages_Total:       0\n";
    assert_eq!(parse_meminfo_kb(text), Some(32695656));
    assert_eq!(parse_meminfo_kb("MemTotal: garbage\n"), None);
    assert_eq!(parse_meminfo_kb("nothing here\n"), None);
}

#[test]
fn parse_nproc_values() {
    assert_eq!(parse_nproc("16"), Some(16));
    assert_eq!(parse_nproc("16\n"), Some(16));
    assert_eq!(parse_nproc("  8  "), Some(8));
    assert_eq!(parse_nproc("0"), None);
    assert_eq!(parse_nproc("abc"), None);
    assert_eq!(parse_nproc(""), None);
}

// ---- format_seconds_display / timeout_exit ----

#[test]
fn format_seconds_display_parity_with_run() {
    assert_eq!(format_seconds_display(2000), "2s");
    assert_eq!(format_seconds_display(2500), "2.5s");
    assert_eq!(format_seconds_display(1234), "1.234s");
    assert_eq!(format_seconds_display(1000), "1s");
    assert_eq!(format_seconds_display(1), "0.001s");
}

#[test]
fn timeout_exit_boundary() {
    assert_eq!(timeout_exit(7, 2000, 2000), Some(124));
    assert_eq!(timeout_exit(7, 5000, 2000), Some(124));
    assert_eq!(timeout_exit(7, 1999, 2000), None);
    assert_eq!(timeout_exit(3, 1, 1), Some(124));
}

// ---- compose goldens ----

#[test]
fn compose_all_flags_exact_argv() {
    let cfg = all_flags_cfg();
    let l = compose(
        &cfg,
        &facts(16, 24 * GIB),
        "me",
        "/mnt/c/work",
        "wnice-42-1000",
    )
    .unwrap();
    assert_eq!(l.unit, "wnice-42-1000");
    assert_eq!(
        l.wsl_args,
        vecs(&[
            "-d",
            "Ubuntu-24.04",
            "-u",
            "root",
            "--cd",
            "/mnt/c/work",
            "--exec",
            "systemd-run",
            "--scope",
            "--quiet",
            "--collect",
            "--unit=wnice-42-1000",
            "-p",
            "CPUQuota=800%",
            "-p",
            "CPUWeight=1",
            "-p",
            "MemoryMax=2147483648",
            "-p",
            "AllowedCPUs=0-3",
            "-p",
            "TasksMax=64",
            "-p",
            "RuntimeMaxSec=2500ms",
            "-p",
            "KillSignal=SIGKILL",
            "--",
            "nice",
            "-n",
            "19",
            "runuser",
            "-u",
            "me",
            "--",
            "sleep",
            "1",
        ])
    );
}

#[test]
fn compose_no_flags_minimal_no_distro_element() {
    let cfg = Config {
        command: vecs(&["true"]),
        ..Default::default()
    };
    let l = compose(&cfg, &facts(0, 0), "me", "/mnt/c/work", "u1").unwrap();
    // No -d anywhere: distro None means wsl.exe's default.
    assert_eq!(
        l.wsl_args,
        vecs(&[
            "-u",
            "root",
            "--cd",
            "/mnt/c/work",
            "--exec",
            "systemd-run",
            "--scope",
            "--quiet",
            "--collect",
            "--unit=u1",
            "--",
            "runuser",
            "-u",
            "me",
            "--",
            "true",
        ])
    );
    assert_eq!(l.unit, "u1");
}

#[test]
fn compose_cpu_percent_of_all_vcpus() {
    let cfg = Config {
        cpu_percent: Some(50),
        command: vecs(&["true"]),
        ..Default::default()
    };
    let l = compose(&cfg, &facts(16, 0), "me", "/mnt/c", "u").unwrap();
    assert!(l.wsl_args.contains(&"CPUQuota=800%".to_string()), "{l:?}");
}

#[test]
fn compose_memory_goldens() {
    // 50 percent of 24 GiB, exact bytes.
    let cfg = Config {
        memory: Some(MemSpec::Percent(50)),
        command: vecs(&["true"]),
        ..Default::default()
    };
    let l = compose(&cfg, &facts(1, 24 * GIB), "me", "/mnt/c", "u").unwrap();
    assert!(
        l.wsl_args.contains(&"MemoryMax=12884901888".to_string()),
        "{l:?}"
    );
    // Bytes pass through as a plain decimal count.
    let cfg = Config {
        memory: Some(MemSpec::Bytes(2 * GIB)),
        command: vecs(&["true"]),
        ..Default::default()
    };
    let l = compose(&cfg, &facts(0, 0), "me", "/mnt/c", "u").unwrap();
    assert!(
        l.wsl_args.contains(&"MemoryMax=2147483648".to_string()),
        "{l:?}"
    );
}

#[test]
fn compose_threads_allowed_cpus() {
    let facts = facts(16, 0);
    let cfg = Config {
        threads: Some(4),
        command: vecs(&["true"]),
        ..Default::default()
    };
    let l = compose(&cfg, &facts, "me", "/mnt/c", "u").unwrap();
    assert!(l.wsl_args.contains(&"AllowedCPUs=0-3".to_string()), "{l:?}");
    let cfg = Config {
        threads: Some(1),
        command: vecs(&["true"]),
        ..Default::default()
    };
    let l = compose(&cfg, &facts, "me", "/mnt/c", "u").unwrap();
    assert!(l.wsl_args.contains(&"AllowedCPUs=0".to_string()), "{l:?}");
}

#[test]
fn compose_priority_variants() {
    for (class, nice, weight) in [
        (PriorityClass::Idle, "19", "1"),
        (PriorityClass::BelowNormal, "10", "50"),
        (PriorityClass::Normal, "0", "100"),
        (PriorityClass::AboveNormal, "-5", "500"),
        (PriorityClass::High, "-20", "10000"),
    ] {
        let cfg = Config {
            priority: Some(class),
            command: vecs(&["true"]),
            ..Default::default()
        };
        let l = compose(&cfg, &facts(0, 0), "me", "/mnt/c", "u").unwrap();
        assert!(
            l.wsl_args
                .windows(2)
                .any(|w| w[0] == "-p" && w[1] == format!("CPUWeight={weight}")),
            "{l:?}"
        );
        let tail = &l.wsl_args[l.wsl_args.len() - 8..];
        assert_eq!(
            tail,
            &vecs(&["nice", "-n", nice, "runuser", "-u", "me", "--", "true"])
        );
    }
}

// ---- compose validation refusals ----

#[test]
fn compose_realtime_refused() {
    let cfg = Config {
        priority: Some(PriorityClass::Realtime),
        command: vecs(&["true"]),
        ..Default::default()
    };
    assert_eq!(
        compose(&cfg, &facts(0, 0), "me", "/mnt/c", "u").unwrap_err(),
        REALTIME_REFUSAL
    );
    // The helper carries the same refusal for direct callers.
    assert_eq!(guest_command(&cfg, "me").unwrap_err(), REALTIME_REFUSAL);
}

#[test]
fn compose_threads_over_vcpu() {
    let cfg = Config {
        threads: Some(17),
        command: vecs(&["true"]),
        ..Default::default()
    };
    let vcpu16 = facts(16, 0);
    assert_eq!(
        compose(&cfg, &vcpu16, "me", "/mnt/c", "u").unwrap_err(),
        "wnice: --wsl: -t/--threads 17 exceeds the guest's 16 vCPUs"
    );
    // Unknown vCPU count (0) is the same refusal with 0 substituted.
    assert_eq!(
        compose(&cfg, &facts(0, 0), "me", "/mnt/c", "u").unwrap_err(),
        "wnice: --wsl: -t/--threads 17 exceeds the guest's 0 vCPUs"
    );
}

#[test]
fn compose_memory_percent_needs_guest_ram() {
    // mem_bytes unknown (0): percent cannot be resolved.
    let cfg = Config {
        memory: Some(MemSpec::Percent(50)),
        command: vecs(&["true"]),
        ..Default::default()
    };
    assert_eq!(
        compose(&cfg, &facts(0, 0), "me", "/mnt/c", "u").unwrap_err(),
        "wnice: --wsl: memory limit is too small - rounds to 0 bytes"
    );
    // 1 percent of 50 bytes floors to 0.
    let tiny = facts(1, 50);
    let cfg = Config {
        memory: Some(MemSpec::Percent(1)),
        command: vecs(&["true"]),
        ..Default::default()
    };
    assert_eq!(
        compose(&cfg, &tiny, "me", "/mnt/c", "u").unwrap_err(),
        "wnice: --wsl: memory limit is too small - rounds to 0 bytes"
    );
}

// ---- metachar safety ----

#[test]
fn compose_metachar_args_stay_verbatim_elements() {
    const TRICKY: [&str; 13] = [
        "a;b",
        "c&d",
        "e|f",
        "$HOME",
        "$(id)",
        "back`tick",
        "q\"q",
        "with space",
        "%s",
        "100%",
        "",
        "Ø—日本",
        "line1\nline2",
    ];
    let mut command = vec!["true".to_string()];
    command.extend(TRICKY.iter().map(|s| s.to_string()));
    let cfg = Config {
        command,
        ..Default::default()
    };
    let l = compose(&cfg, &facts(0, 0), "me", "/mnt/c/work", "u1").unwrap();
    // Minimal construction is 15 elements, then argv0 + every tricky arg.
    let expected_len = 15 + 1 + TRICKY.len();
    assert_eq!(l.wsl_args.len(), expected_len, "{l:?}");
    // Prefix must be exactly the minimal construction (no shell, no -d).
    assert_eq!(
        &l.wsl_args[..15],
        &vecs(&[
            "-u",
            "root",
            "--cd",
            "/mnt/c/work",
            "--exec",
            "systemd-run",
            "--scope",
            "--quiet",
            "--collect",
            "--unit=u1",
            "--",
            "runuser",
            "-u",
            "me",
            "--",
        ])
    );
    // Every tricky arg appears VERBATIM as its own element, in order.
    let mut expected_tail = vec!["true".to_string()];
    expected_tail.extend(TRICKY.iter().map(|s| s.to_string()));
    assert_eq!(&l.wsl_args[15..], &expected_tail);
    // No shell element anywhere in the assembled argv.
    assert!(l
        .wsl_args
        .iter()
        .all(|e| e != "sh" && e != "bash" && e != "cmd" && e != "cmd.exe"));
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
fn flow_default_distro_has_no_dash_d() {
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
    assert!(calls.iter().all(|c| !c.args.contains(&"-d".to_string())));
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
