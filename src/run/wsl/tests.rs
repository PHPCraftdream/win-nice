#![cfg(test)]
//! Pure-logic, compose-golden and metachar-safety tests for WSL mode.
//! Scripted-flow tests (a fake `wsl.exe` driver, no WSL required) live in
//! `flow_tests.rs`.

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

/// Real-looking decoded `wsl -l -v` output: one starred default distro.
fn distro_list_text() -> String {
    "  NAME                   STATE           VERSION\r\n\
     * Ubuntu-24.04           Running         2\r\n\
       Debian-12              Stopped         2\r\n"
        .to_string()
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
        audit: false,
        no_servers: false,
        command: vecs(&["sleep", "1"]),
    }
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
    // canonicalize/current_dir can return the verbatim drive form.
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
    // Verbatim-UNC and bare-UNC forms refuse (regressions: a verbatim
    // DRIVE prefix `\\?\C:\x` must translate, only UNC shapes refuse).
    for p in [r"\\?\UNC\srv\share", r"\\srv\share"] {
        let e = translate_windows_cwd(p).unwrap_err();
        assert!(e.contains("UNC paths are not supported"), "{e}");
    }
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
