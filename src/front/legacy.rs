//! Legacy single-purpose frontends, selected by argv[0]: `idle`..`realtime`
//! (`<tool> <command> [args...]`) and `capc`/`capt`/`capm`/`caps`/`capn`
//! (`<tool> <value> <command> [args...]`). Positional syntax only — nothing
//! after the value is option-parsed — mapped onto the combined `wnice`
//! [`Config`]; values are validated by the `wnice` parser itself.

use crate::cli::{parse_args, Action};
use crate::{Config, PriorityClass};

const CAPM_USAGE: &str = "usage: capm <size> <command> [args...]  (size: plain integer 1-100 \
= percent of total physical RAM, e.g. 50 - same convention as capc's <percent 1-100>; \
number+m/M = MB, e.g. 512m; number+g/G = GB, e.g. 2g)";
const CAPS_USAGE: &str = "usage: caps <seconds> <command> [args...]  (seconds: positive whole \
or decimal number, e.g. 2 or 2.5 - converted to whole milliseconds; minimum 1 ms, maximum \
4294967294 ms (~49.7 days) - a deliberate ceiling, not an API limit)";
const CAPN_USAGE: &str = "usage: capn <count> <command> [args...]  (count: positive whole \
number, minimum 1; maximum 4294967295, because ActiveProcessLimit is a uint32 struct field)";

fn priority_of(stem: &str) -> Option<PriorityClass> {
    match stem {
        "idle" => Some(PriorityClass::Idle),
        "belownormal" => Some(PriorityClass::BelowNormal),
        "abovenormal" => Some(PriorityClass::AboveNormal),
        "high" => Some(PriorityClass::High),
        "realtime" => Some(PriorityClass::Realtime),
        _ => None,
    }
}

/// `wnice` flag and usage text for a value-taking legacy tool.
fn value_tool(stem: &str, logical_cpus: u32) -> Option<(&'static str, String)> {
    let max_threads = logical_cpus.clamp(1, 63);
    Some(match stem {
        "capc" => (
            "-c",
            "usage: capc <percent 1-100> <command> [args...]".to_string(),
        ),
        "capt" => (
            "-t",
            format!("usage: capt <thread-count 1-{max_threads}> <command> [args...]"),
        ),
        "capm" => ("-m", CAPM_USAGE.to_string()),
        "caps" => ("-s", CAPS_USAGE.to_string()),
        "capn" => ("-n", CAPN_USAGE.to_string()),
        _ => return None,
    })
}

/// True when `stem` names a frontend handled here.
pub fn is_legacy(stem: &str) -> bool {
    priority_of(stem).is_some() || value_tool(stem, 1).is_some()
}

/// Parse a legacy frontend's arguments (argv[0] excluded). `Err` is the
/// complete stderr message for a usage error (exit 1).
pub fn parse(stem: &str, args: &[String], logical_cpus: u32) -> Result<Config, String> {
    if let Some(p) = priority_of(stem) {
        if args.is_empty() {
            return Err(format!("usage: {stem} <command> [args...]"));
        }
        return Ok(Config {
            priority: Some(p),
            command: args.to_vec(),
            ..Config::default()
        });
    }
    let (flag, usage) = value_tool(stem, logical_cpus)
        .unwrap_or_else(|| panic!("legacy::parse called for non-legacy '{stem}'"));
    if args.len() < 2 {
        return Err(usage);
    }
    let probe = [
        flag.to_string(),
        args[0].clone(),
        "--".to_string(),
        "x".to_string(),
    ];
    match parse_args(&probe, logical_cpus) {
        Ok(Action::Run(mut cfg)) => {
            cfg.command = args[1..].to_vec();
            Ok(cfg)
        }
        _ => Err(usage),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemSpec;

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn priority_tools_take_everything_as_the_command() {
        let c = parse("idle", &a(&["cmd", "-c", "50", "--"]), 8).unwrap();
        assert_eq!(c.priority, Some(PriorityClass::Idle));
        assert_eq!(c.command, a(&["cmd", "-c", "50", "--"]));
        assert!(c.cpu_percent.is_none());
        for (stem, p) in [
            ("belownormal", PriorityClass::BelowNormal),
            ("abovenormal", PriorityClass::AboveNormal),
            ("high", PriorityClass::High),
            ("realtime", PriorityClass::Realtime),
        ] {
            assert_eq!(parse(stem, &a(&["x"]), 8).unwrap().priority, Some(p));
        }
    }

    #[test]
    fn priority_tool_without_command_is_usage_error() {
        assert_eq!(
            parse("high", &[], 8).unwrap_err(),
            "usage: high <command> [args...]"
        );
    }

    #[test]
    fn value_tools_map_onto_wnice_limits() {
        assert_eq!(
            parse("capc", &a(&["50", "cmd"]), 8).unwrap().cpu_percent,
            Some(50)
        );
        assert_eq!(
            parse("capt", &a(&["2", "cmd"]), 8).unwrap().threads,
            Some(2)
        );
        assert_eq!(
            parse("capm", &a(&["512m", "cmd"]), 8).unwrap().memory,
            Some(MemSpec::Bytes(512 * 1024 * 1024))
        );
        assert_eq!(
            parse("capm", &a(&["25", "cmd"]), 8).unwrap().memory,
            Some(MemSpec::Percent(25))
        );
        assert_eq!(
            parse("caps", &a(&["2.5", "cmd"]), 8).unwrap().timeout_ms,
            Some(2500)
        );
        assert_eq!(parse("capn", &a(&["3", "cmd"]), 8).unwrap().procs, Some(3));
    }

    #[test]
    fn value_tool_command_is_verbatim_even_when_it_looks_like_options() {
        let c = parse("capc", &a(&["50", "-p", "high", "--", "y"]), 8).unwrap();
        assert_eq!(c.command, a(&["-p", "high", "--", "y"]));
        assert!(c.priority.is_none());
    }

    #[test]
    fn invalid_or_missing_values_give_the_tool_usage() {
        let usage = "usage: capc <percent 1-100> <command> [args...]";
        assert_eq!(parse("capc", &a(&["0", "cmd"]), 8).unwrap_err(), usage);
        assert_eq!(parse("capc", &a(&["101", "cmd"]), 8).unwrap_err(), usage);
        assert_eq!(parse("capc", &a(&["50"]), 8).unwrap_err(), usage);
        assert_eq!(
            parse("capt", &a(&["9", "cmd"]), 8).unwrap_err(),
            "usage: capt <thread-count 1-8> <command> [args...]"
        );
        assert_eq!(
            parse("capm", &a(&["50.5", "cmd"]), 8).unwrap_err(),
            CAPM_USAGE
        );
        assert_eq!(
            parse("capm", &a(&["25%", "cmd"]), 8).unwrap_err(),
            CAPM_USAGE
        );
        assert_eq!(parse("caps", &a(&["0", "cmd"]), 8).unwrap_err(), CAPS_USAGE);
        assert_eq!(
            parse("caps", &a(&["-1", "cmd"]), 8).unwrap_err(),
            CAPS_USAGE
        );
        assert_eq!(parse("capn", &a(&["0", "cmd"]), 8).unwrap_err(), CAPN_USAGE);
        assert_eq!(
            parse("capn", &a(&["4294967296", "cmd"]), 8).unwrap_err(),
            CAPN_USAGE
        );
    }

    #[test]
    fn is_legacy_covers_exactly_the_ten_frontends() {
        for s in [
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
        ] {
            assert!(is_legacy(s), "{s}");
        }
        for s in ["wnice", "cy", "cx", "admin", "uiup", "normal"] {
            assert!(!is_legacy(s), "{s}");
        }
    }
}
