//! Hand-written argument parser for the combined `wnice` frontend.
//!
//! Options are parsed ONLY before the first non-option token or `--`;
//! everything after belongs verbatim to the wrapped command.

use crate::{Config, MemSpec, PriorityClass};

pub const USAGE: &str = "\
usage: wnice [options] [--] <command> [args...]
options:
  -p, --priority <idle|belownormal|normal|abovenormal|high|realtime>
  -c, --cpu <1-100>
  -t, --threads <1..min(logical processors, 63)>
  -m, --memory <size>   (<1-100> percent of RAM, or <n>[.d](m|g) bytes, e.g. 512m, 0.5g)
  -n, --procs <1-4294967295>
  -s, --timeout <seconds>  (decimal seconds > 0, floored to whole ms, max 4294967294 ms)
  -h, --help
  -V, --version
options end at the first non-option token or at '--'; everything after
belongs to the wrapped command.";

/// Outcome of parsing the `wnice` command line.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Run(Config),
    Help,
    Version,
}

fn strict_u32(s: &str) -> Option<u32> {
    // Strict decimal integer, no sign, no whitespace, no overflow.
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if s.len() > 10 {
        return None;
    }
    s.parse::<u32>().ok()
}

fn parse_priority(s: &str) -> Option<PriorityClass> {
    match s.to_ascii_lowercase().as_str() {
        "idle" => Some(PriorityClass::Idle),
        "belownormal" => Some(PriorityClass::BelowNormal),
        "normal" => Some(PriorityClass::Normal),
        "abovenormal" => Some(PriorityClass::AboveNormal),
        "high" => Some(PriorityClass::High),
        "realtime" => Some(PriorityClass::Realtime),
        _ => None,
    }
}

/// `<size>`: grammar identical to bin/capm.ps1 — `^(\d+(\.\d+)?)([mMgG]?)$`.
fn parse_memory(s: &str) -> Result<MemSpec, String> {
    let bad = || "invalid -m/--memory <size>".to_string();
    // Split off the optional single m/M/g/G suffix.
    let bytes = s.as_bytes();
    let (num, unit) = match bytes.last() {
        Some(b'm') | Some(b'M') | Some(b'g') | Some(b'G') => {
            (&s[..s.len() - 1], bytes[bytes.len() - 1])
        }
        _ => (s, 0),
    };
    // num must match \d+(\.\d+)?
    let (int_part, frac_part) = match num.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (num, None),
    };
    let digits_ok = |x: &str| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit());
    if !digits_ok(int_part) || frac_part.is_some_and(|f| !digits_ok(f)) {
        return Err(bad());
    }
    if unit == 0 {
        // Bare number: percent of RAM, plain integer only (like capm.ps1's
        // [int]::TryParse — rejects "50.5" and overflow).
        let n = strict_u32(num).ok_or_else(bad)?;
        if !(1..=100).contains(&n) {
            return Err(bad());
        }
        return Ok(MemSpec::Percent(n));
    }
    // With a unit: value must be > 0; compute bytes exactly in u128.
    let mult: u128 = if unit == b'm' || unit == b'M' {
        1024 * 1024
    } else {
        1024 * 1024 * 1024
    };
    let int_val: u128 = if int_part.len() > 30 {
        return Err("invalid -m/--memory <size>: out of range".to_string());
    } else {
        int_part.parse::<u128>().unwrap_or(u128::MAX)
    };
    let mut total = int_val
        .checked_mul(mult)
        .ok_or_else(|| "invalid -m/--memory <size>: out of range".to_string())?;
    if let Some(f) = frac_part {
        // frac * mult / 10^len, exact floor.
        let f_val: u128 = if f.len() > 30 {
            return Err("invalid -m/--memory <size>: out of range".to_string());
        } else {
            f.parse::<u128>().unwrap_or(u128::MAX)
        };
        let denom = 10u128
            .checked_pow(f.len() as u32)
            .ok_or_else(|| "invalid -m/--memory <size>: out of range".to_string())?;
        let frac_bytes = f_val
            .checked_mul(mult)
            .ok_or_else(|| "invalid -m/--memory <size>: out of range".to_string())?
            / denom;
        total = total
            .checked_add(frac_bytes)
            .ok_or_else(|| "invalid -m/--memory <size>: out of range".to_string())?;
    }
    if total == 0 {
        return Err(bad());
    }
    if total > u64::MAX as u128 {
        return Err("invalid -m/--memory <size>: out of range".to_string());
    }
    Ok(MemSpec::Bytes(total as u64))
}

/// `<seconds>`: `^\d+(\.\d+)?$`, > 0, floored to whole ms in [1, 4294967294].
/// Parsed as integer + fraction digits to avoid f64 precision problems.
fn parse_timeout(s: &str) -> Result<u32, String> {
    let bad = || "invalid -s/--timeout <seconds>".to_string();
    let (int_part, frac_part) = match s.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (s, None),
    };
    let digits_ok = |x: &str| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit());
    if !digits_ok(int_part) || frac_part.is_some_and(|f| !digits_ok(f)) {
        return Err(bad());
    }
    let int_val: u64 = if int_part.len() > 15 {
        return Err(bad());
    } else {
        int_part.parse::<u64>().unwrap_or(u64::MAX)
    };
    let int_ms = int_val.checked_mul(1000).ok_or_else(bad)?;
    let frac_ms: u64 = match frac_part {
        None => 0,
        Some(f) => {
            // Floor: take the first three fraction digits (pad with zeros);
            // further digits cannot raise the floored millisecond value.
            let mut d: u64 = 0;
            for i in 0..3 {
                d = d * 10 + f.as_bytes().get(i).map_or(0, |b| (b - b'0') as u64);
            }
            d
        }
    };
    let total_ms = int_ms.checked_add(frac_ms).ok_or_else(bad)?;
    if !(1..=4294967294).contains(&total_ms) {
        return Err(bad());
    }
    Ok(total_ms as u32)
}

/// Parse the `wnice` argument list (excluding argv[0]).
///
/// `logical_cpus` is the machine's logical processor count, used as the upper
/// bound for `-t/--threads`.
pub fn parse_args(args: &[String], logical_cpus: u32) -> Result<Action, String> {
    let mut cfg = Config::default();
    let mut want_help = false;
    let mut want_version = false;
    let mut i = 0usize;
    let mut options_done = false;

    while i < args.len() {
        let arg = &args[i];
        if options_done {
            cfg.command.push(arg.clone());
            i += 1;
            continue;
        }
        if arg == "--" {
            options_done = true;
            i += 1;
            continue;
        }
        if arg == "-h" || arg == "--help" {
            want_help = true;
            i += 1;
            continue;
        }
        if arg == "-V" || arg == "--version" {
            want_version = true;
            i += 1;
            continue;
        }
        let flag: &str = if arg == "-p" || arg == "--priority" {
            "-p/--priority"
        } else if arg == "-c" || arg == "--cpu" {
            "-c/--cpu"
        } else if arg == "-t" || arg == "--threads" {
            "-t/--threads"
        } else if arg == "-m" || arg == "--memory" {
            "-m/--memory"
        } else if arg == "-n" || arg == "--procs" {
            "-n/--procs"
        } else if arg == "-s" || arg == "--timeout" {
            "-s/--timeout"
        } else if arg.starts_with('-') && arg.len() > 1 {
            return Err(format!("unknown option '{arg}'"));
        } else {
            // First non-option token: the command starts here.
            options_done = true;
            continue;
        };
        let Some(val) = args.get(i + 1) else {
            return Err(format!("missing value for {flag}"));
        };
        i += 2;
        match flag {
            "-p/--priority" => {
                cfg.priority = Some(
                    parse_priority(val)
                        .ok_or_else(|| format!("invalid -p/--priority value '{val}'"))?,
                );
            }
            "-c/--cpu" => {
                let n = strict_u32(val).ok_or_else(|| format!("invalid -c/--cpu value '{val}'"))?;
                if !(1..=100).contains(&n) {
                    return Err(format!("invalid -c/--cpu value '{val}'"));
                }
                cfg.cpu_percent = Some(n);
            }
            "-t/--threads" => {
                let n =
                    strict_u32(val).ok_or_else(|| format!("invalid -t/--threads value '{val}'"))?;
                let max = logical_cpus.clamp(1, 63);
                if n < 1 || n > max {
                    return Err(format!("invalid -t/--threads value '{val}'"));
                }
                cfg.threads = Some(n);
            }
            "-m/--memory" => {
                cfg.memory = Some(parse_memory(val)?);
            }
            "-n/--procs" => {
                let n =
                    strict_u32(val).ok_or_else(|| format!("invalid -n/--procs value '{val}'"))?;
                if n == 0 {
                    return Err(format!("invalid -n/--procs value '{val}'"));
                }
                cfg.procs = Some(n);
            }
            "-s/--timeout" => {
                cfg.timeout_ms = Some(parse_timeout(val)?);
            }
            _ => unreachable!(),
        }
    }

    if want_help {
        return Ok(Action::Help);
    }
    if want_version {
        return Ok(Action::Version);
    }
    if cfg.command.is_empty() {
        return Err("no command given".to_string());
    }
    Ok(Action::Run(cfg))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn run(v: &[&str]) -> Config {
        match parse_args(&args(v), 8).ok().unwrap() {
            Action::Run(c) => c,
            other => panic!("expected Run, got {other:?}"),
        }
    }

    fn err(v: &[&str]) -> String {
        parse_args(&args(v), 8).unwrap_err()
    }

    #[test]
    fn empty_command_rejected() {
        assert!(parse_args(&[], 8).is_err());
        assert!(parse_args(&args(&["-c", "50"]), 8).is_err());
        assert!(parse_args(&args(&["--"]), 8).is_err());
    }

    #[test]
    fn plain_command() {
        let c = run(&["cmd"]);
        assert!(c.priority.is_none());
        assert_eq!(c.command, vec!["cmd"]);
    }

    #[test]
    fn all_options_valid() {
        let c = run(&[
            "-p", "HIGH", "-c", "50", "-t", "4", "-m", "512m", "-n", "10", "-s", "5", "cmd", "arg1",
        ]);
        assert_eq!(c.priority, Some(PriorityClass::High));
        assert_eq!(c.cpu_percent, Some(50));
        assert_eq!(c.threads, Some(4));
        assert_eq!(c.memory, Some(MemSpec::Bytes(512 * 1024 * 1024)));
        assert_eq!(c.procs, Some(10));
        assert_eq!(c.timeout_ms, Some(5000));
        assert_eq!(c.command, vec!["cmd", "arg1"]);
    }

    #[test]
    fn long_forms() {
        let c = run(&[
            "--priority",
            "idle",
            "--cpu",
            "1",
            "--threads",
            "8",
            "--memory",
            "50",
            "--procs",
            "1",
            "--timeout",
            "1",
            "cmd",
        ]);
        assert_eq!(c.priority, Some(PriorityClass::Idle));
        assert_eq!(c.memory, Some(MemSpec::Percent(50)));
        assert_eq!(c.threads, Some(8));
    }

    #[test]
    fn double_dash_ends_options() {
        let c = run(&["-c", "10", "--", "-p", "idle"]);
        assert_eq!(c.cpu_percent, Some(10));
        assert!(c.priority.is_none());
        assert_eq!(c.command, vec!["-p", "idle"]);
    }

    #[test]
    fn flag_after_command_not_consumed() {
        let c = run(&["cmd", "-p", "idle"]);
        assert!(c.priority.is_none());
        assert_eq!(c.command, vec!["cmd", "-p", "idle"]);
    }

    #[test]
    fn everything_after_command_verbatim() {
        let c = run(&["-p", "idle", "cmd", "-c", "999", "--whatever"]);
        assert_eq!(c.priority, Some(PriorityClass::Idle));
        assert_eq!(c.command, vec!["cmd", "-c", "999", "--whatever"]);
    }

    #[test]
    fn abbreviations_rejected() {
        assert!(!err(&["--pri", "idle", "cmd"]).is_empty());
        assert!(!err(&["--cpu", "50", "--pr", "high", "cmd"]).is_empty());
    }

    #[test]
    fn equals_forms_rejected() {
        assert!(!err(&["--priority=abc", "cmd"]).is_empty());
        assert!(!err(&["-p=abc", "cmd"]).is_empty());
        assert!(!err(&["--cpu=50", "cmd"]).is_empty());
    }

    #[test]
    fn unknown_option_rejected() {
        assert!(!err(&["-x", "cmd"]).is_empty());
        assert!(!err(&["--bogus", "cmd"]).is_empty());
        assert!(!err(&["-1", "cmd"]).is_empty());
    }

    #[test]
    fn missing_value_rejected() {
        assert!(!err(&["-p"]).is_empty());
        assert!(!err(&["-c"]).is_empty());
        assert!(!err(&["-m"]).is_empty());
        assert!(!err(&["--timeout"]).is_empty());
    }

    #[test]
    fn priority_values() {
        for (v, want) in [
            ("idle", PriorityClass::Idle),
            ("IDLE", PriorityClass::Idle),
            ("BelowNormal", PriorityClass::BelowNormal),
            ("normal", PriorityClass::Normal),
            ("abovenormal", PriorityClass::AboveNormal),
            ("ABOVENORMAL", PriorityClass::AboveNormal),
            ("High", PriorityClass::High),
            ("REALTIME", PriorityClass::Realtime),
        ] {
            let c = run(&["-p", v, "cmd"]);
            assert_eq!(c.priority, Some(want), "value {v}");
        }
        assert!(!err(&["-p", "bogus", "cmd"]).is_empty());
        assert!(!err(&["-p", "", "cmd"]).is_empty());
    }

    #[test]
    fn cpu_boundaries() {
        assert_eq!(run(&["-c", "1", "cmd"]).cpu_percent, Some(1));
        assert_eq!(run(&["-c", "100", "cmd"]).cpu_percent, Some(100));
        assert!(!err(&["-c", "0", "cmd"]).is_empty());
        assert!(!err(&["-c", "101", "cmd"]).is_empty());
        assert!(!err(&["-c", "50.5", "cmd"]).is_empty());
        assert!(!err(&["-c", " 50", "cmd"]).is_empty());
        assert!(!err(&["-c", "+50", "cmd"]).is_empty());
        assert!(!err(&["-c", "0x32", "cmd"]).is_empty());
        let big = "9".repeat(400);
        assert!(!err(&["-c", big.as_str(), "cmd"]).is_empty());
    }

    #[test]
    fn threads_boundaries() {
        assert_eq!(run(&["-t", "1", "cmd"]).threads, Some(1));
        assert_eq!(run(&["-t", "8", "cmd"]).threads, Some(8));
        // min(128, 63) = 63 with a large logical_cpus.
        assert_eq!(
            parse_args(&args(&["-t", "63", "cmd"]), 128).ok().unwrap(),
            Action::Run(Config {
                threads: Some(63),
                command: vec!["cmd".into()],
                ..Default::default()
            })
        );
        // logical_cpus=8 caps at 8.
        assert!(!err(&["-t", "9", "cmd"]).is_empty());
        // Above the 63 parse-limit even with a large logical_cpus.
        assert!(parse_args(&args(&["-t", "64", "cmd"]), 128).is_err());
        assert!(parse_args(&args(&["-t", "0", "cmd"]), 8).is_err());
    }

    #[test]
    fn memory_grammar() {
        assert_eq!(run(&["-m", "50", "cmd"]).memory, Some(MemSpec::Percent(50)));
        assert_eq!(
            run(&["-m", "100", "cmd"]).memory,
            Some(MemSpec::Percent(100))
        );
        assert_eq!(run(&["-m", "1", "cmd"]).memory, Some(MemSpec::Percent(1)));
        assert!(!err(&["-m", "101", "cmd"]).is_empty());
        assert!(!err(&["-m", "0", "cmd"]).is_empty());
        assert!(!err(&["-m", "50.5", "cmd"]).is_empty());
        assert_eq!(
            run(&["-m", "512m", "cmd"]).memory,
            Some(MemSpec::Bytes(512 * 1024 * 1024))
        );
        assert_eq!(
            run(&["-m", "512M", "cmd"]).memory,
            Some(MemSpec::Bytes(512 * 1024 * 1024))
        );
        assert_eq!(
            run(&["-m", "2g", "cmd"]).memory,
            Some(MemSpec::Bytes(2 * 1024 * 1024 * 1024))
        );
        assert_eq!(
            run(&["-m", "0.5g", "cmd"]).memory,
            Some(MemSpec::Bytes(536_870_912))
        );
        assert!(!err(&["-m", "0m", "cmd"]).is_empty());
        assert!(!err(&["-m", "0.0m", "cmd"]).is_empty());
        assert!(!err(&["-m", "50x", "cmd"]).is_empty());
        assert!(!err(&["-m", "512kb", "cmd"]).is_empty());
        let big = "9".repeat(400);
        assert!(!err(&["-m", big.as_str(), "cmd"]).is_empty());
        let big_g = format!("{}g", "9".repeat(400));
        assert!(!err(&["-m", big_g.as_str(), "cmd"]).is_empty());
    }

    #[test]
    fn procs_values() {
        assert_eq!(run(&["-n", "1", "cmd"]).procs, Some(1));
        assert_eq!(run(&["-n", "4294967295", "cmd"]).procs, Some(u32::MAX));
        assert!(!err(&["-n", "4294967296", "cmd"]).is_empty());
        assert!(!err(&["-n", "0", "cmd"]).is_empty());
        assert!(!err(&["-n", "-1", "cmd"]).is_empty());
        let big = "9".repeat(400);
        assert!(!err(&["-n", big.as_str(), "cmd"]).is_empty());
    }

    #[test]
    fn timeout_values() {
        assert_eq!(run(&["-s", "2", "cmd"]).timeout_ms, Some(2000));
        assert_eq!(run(&["-s", "2.5", "cmd"]).timeout_ms, Some(2500));
        assert_eq!(run(&["-s", "1", "cmd"]).timeout_ms, Some(1000));
        assert_eq!(run(&["-s", "0.001", "cmd"]).timeout_ms, Some(1));
        // Floors: 0.0009s = 0.9ms -> 0 -> fail.
        assert!(!err(&["-s", "0.0009", "cmd"]).is_empty());
        assert!(!err(&["-s", "0", "cmd"]).is_empty());
        assert!(!err(&["-s", "0.0", "cmd"]).is_empty());
        assert!(!err(&["-s", "-1", "cmd"]).is_empty());
        // Upper boundary: exactly 4294967294 ms.
        assert_eq!(
            run(&["-s", "4294967.294", "cmd"]).timeout_ms,
            Some(4294967294)
        );
        assert!(!err(&["-s", "4294967.295", "cmd"]).is_empty());
        assert!(!err(&["-s", "4294967295", "cmd"]).is_empty());
        let big = "9".repeat(400);
        assert!(!err(&["-s", big.as_str(), "cmd"]).is_empty());
        assert!(!err(&["-s", "abc", "cmd"]).is_empty());
        assert!(!err(&["-s", "1.2.3", "cmd"]).is_empty());
    }

    #[test]
    fn help_short_circuits() {
        assert_eq!(parse_args(&args(&["-h"]), 8).ok(), Some(Action::Help));
        assert_eq!(parse_args(&args(&["--help"]), 8).ok(), Some(Action::Help));
        assert_eq!(
            parse_args(&args(&["-c", "50", "--help", "cmd"]), 8).ok(),
            Some(Action::Help)
        );
    }

    #[test]
    fn version_short_circuits() {
        assert_eq!(parse_args(&args(&["-V"]), 8).ok(), Some(Action::Version));
        assert_eq!(
            parse_args(&args(&["--version"]), 8).ok(),
            Some(Action::Version)
        );
        assert_eq!(
            parse_args(&args(&["cmd", "--version"]), 8).ok(),
            Some(Action::Run(Config {
                command: vec!["cmd".into(), "--version".into()],
                ..Default::default()
            }))
        );
    }

    #[test]
    fn no_clustering() {
        // -pc is not a known option; it must be rejected.
        assert!(!err(&["-pc", "idle", "cmd"]).is_empty());
    }

    #[test]
    fn duplicate_options_last_wins() {
        let c = run(&["-c", "10", "-c", "20", "cmd"]);
        assert_eq!(c.cpu_percent, Some(20));
    }
}
