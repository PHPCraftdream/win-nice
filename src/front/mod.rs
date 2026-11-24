//! Single-purpose frontends selected by `argv[0]` (the hardlinks postinstall
//! creates next to wnice.exe).

pub mod admin;
pub mod help;
pub mod jobs;
pub mod legacy;
mod native;
pub mod uiup;

use crate::{Config, RealSys, Sys};

/// `cy`/`cx`: the AI CLI with its permission prompts disabled, plus the
/// caller's arguments verbatim.
fn agent_prefix(stem: &str) -> Option<[&'static str; 2]> {
    match stem {
        "cy" => Some(["claude", "--dangerously-skip-permissions"]),
        "cx" => Some(["codex", "--dangerously-bypass-approvals-and-sandbox"]),
        _ => None,
    }
}

/// Optional limits for the agent launchers: when set, the `wnice` limit
/// options the agent they start runs under. Whitespace-separated tokens, no
/// quoting.
pub const AGENT_LIMITS_VAR: &str = "WNICE_AGENT_LIMITS";

pub fn audit_env_enabled(value: Option<&str>) -> bool {
    value == Some("1")
}

const AGENT_LIMITS_ONLY: &str =
    "only wnice options (-p, -c, -t, -m, -n, -s, --boost, --no-servers)";

/// Tokens the variable must never carry: everything a `wnice` command line
/// could use to do more than set limits.
fn forbidden_token(token: &str) -> Option<String> {
    let forbidden = matches!(
        token,
        "--" | "--wsl" | "--jobs" | "-h" | "--help" | "-V" | "--version" | "--audit"
    ) || token.starts_with("--wsl=");
    forbidden.then(|| format!("'{token}' is not allowed; {AGENT_LIMITS_ONLY}"))
}

/// Limits from the variable's value: whitespace-separated `wnice` limit
/// options and nothing else. `None` when the value is absent or holds only
/// whitespace (the 0.4.0 behaviour); `Err` is the reason, without the
/// variable name.
fn parse_agent_limits(raw: Option<&str>, logical_cpus: u32) -> Result<Option<Config>, String> {
    let Some(raw) = raw.filter(|v| !v.trim().is_empty()) else {
        return Ok(None);
    };
    let tokens: Vec<String> = raw.split_whitespace().map(str::to_string).collect();
    if let Some(reason) = tokens.iter().find_map(|t| forbidden_token(t)) {
        return Err(reason);
    }
    // The real `wnice` parser: same values, same messages.
    let cfg =
        crate::cli::parse_options(&tokens, logical_cpus).map_err(|e| e.message().to_string())?;
    match cfg.command.first() {
        Some(first) => Err(format!("'{first}' is not an option; {AGENT_LIMITS_ONLY}")),
        None => Ok(Some(cfg)),
    }
}

/// Run the frontend named `stem`; `None` when `stem` is not one.
pub fn dispatch(stem: &str, args: &[String]) -> Option<i32> {
    let sys = std::sync::Arc::new(RealSys);
    if help::wants_help(args) {
        if let Some(text) = help::text(stem) {
            println!("{text}");
            return Some(0);
        }
    }
    if legacy::is_legacy(stem) {
        return Some(
            match legacy::parse(stem, args, sys.logical_processor_count()) {
                Ok(mut cfg) => {
                    cfg.audit = audit_env_enabled(std::env::var("WNICE_AUDIT").ok().as_deref());
                    crate::run::run(&cfg, sys.clone())
                }
                Err(usage) => {
                    eprintln!("{usage}");
                    eprintln!("{}", help::hint(stem));
                    1
                }
            },
        );
    }
    if let Some(prefix) = agent_prefix(stem) {
        let raw = match std::env::var(AGENT_LIMITS_VAR) {
            Ok(v) => Some(v),
            Err(std::env::VarError::NotPresent) => None,
            Err(std::env::VarError::NotUnicode(_)) => {
                eprintln!("{AGENT_LIMITS_VAR}: value is not valid Unicode");
                return Some(1);
            }
        };
        let limits = match parse_agent_limits(raw.as_deref(), sys.logical_processor_count()) {
            Ok(limits) => limits,
            Err(reason) => {
                eprintln!("{AGENT_LIMITS_VAR}: {reason}");
                return Some(1);
            }
        };
        let mut command: Vec<String> = prefix.iter().map(|s| s.to_string()).collect();
        command.extend_from_slice(args);
        let cfg = Config {
            command,
            ..limits.unwrap_or_default()
        };
        return Some(crate::run::run(&cfg, sys.clone()));
    }
    match stem {
        "admin" => Some(admin::main(args)),
        "uiup" => Some(uiup::main(args)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemSpec, PriorityClass};

    #[test]
    fn audit_environment_is_exact_one() {
        assert!(!audit_env_enabled(None));
        assert!(audit_env_enabled(Some("1")));
        for value in ["", "0", "true", " 1", "1 ", "01"] {
            assert!(!audit_env_enabled(Some(value)));
        }
    }

    #[test]
    fn agent_prefixes() {
        assert_eq!(
            agent_prefix("cy"),
            Some(["claude", "--dangerously-skip-permissions"])
        );
        assert_eq!(
            agent_prefix("cx"),
            Some(["codex", "--dangerously-bypass-approvals-and-sandbox"])
        );
        assert_eq!(agent_prefix("wnice"), None);
    }

    fn limits(raw: &str) -> Config {
        parse_agent_limits(Some(raw), 8)
            .unwrap_or_else(|e| panic!("{raw}: {e}"))
            .unwrap_or_else(|| panic!("{raw}: expected limits"))
    }

    fn limits_err(raw: &str) -> String {
        parse_agent_limits(Some(raw), 8).expect_err("expected an error")
    }

    #[test]
    fn agent_limits_unset_or_blank_is_no_limits() {
        assert_eq!(parse_agent_limits(None, 8).unwrap(), None);
        assert_eq!(parse_agent_limits(Some(""), 8).unwrap(), None);
        assert_eq!(parse_agent_limits(Some(" \t\r\n "), 8).unwrap(), None);
    }

    #[test]
    fn agent_limits_take_every_wnice_limit_option() {
        let c = limits("-p idle -c 10 -t 4 -m 10g -n 3 -s 2.5");
        assert_eq!(c.priority, Some(PriorityClass::Idle));
        assert_eq!(c.cpu_percent, Some(10));
        assert_eq!(c.threads, Some(4));
        assert_eq!(c.memory, Some(MemSpec::Bytes(10 * 1024 * 1024 * 1024)));
        assert_eq!(c.procs, Some(3));
        assert_eq!(c.timeout_ms, Some(2500));
        assert!(!c.boost);
        assert!(c.wsl.is_none());
        assert!(c.command.is_empty());

        let c = limits("--priority high --cpu 50 --boost  ");
        assert_eq!(c.priority, Some(PriorityClass::High));
        assert_eq!(c.cpu_percent, Some(50));
        assert!(c.boost);
        assert!(c.command.is_empty());

        let c = limits("--no-servers");
        assert!(c.no_servers);
        assert!(c.command.is_empty());

        // Percent memory and sub-second timeout: exactly what wnice accepts.
        let c = limits("  -m 25\t-s 0.001 ");
        assert_eq!(c.memory, Some(MemSpec::Percent(25)));
        assert_eq!(c.timeout_ms, Some(1));
    }

    #[test]
    fn agent_limits_refuse_every_forbidden_token() {
        assert_eq!(
            forbidden_token("--audit").as_deref(),
            Some("'--audit' is not allowed; only wnice options (-p, -c, -t, -m, -n, -s, --boost, --no-servers)")
        );
        for bad in [
            "--",
            "--jobs",
            "--wsl",
            "--wsl=Debian-12",
            "--help",
            "-h",
            "--version",
            "-V",
            "--audit",
        ] {
            let raw = format!("-c 20 {bad}");
            let err = limits_err(&raw);
            assert!(err.starts_with(&format!("'{bad}'")), "{raw}: {err}");
            assert!(err.contains("not allowed"), "{raw}: {err}");
            assert!(err.contains(AGENT_LIMITS_ONLY), "{raw}: {err}");
            assert!(!err.contains('\n'), "{raw}: one line only");
        }
        for bad in ["--", "--help", "--version"] {
            assert!(limits_err(bad).contains("not allowed"), "{bad}");
        }
    }

    #[test]
    fn agent_limits_refuse_a_command_token() {
        for raw in ["claude", "-c 20 claude", "-c 20 x y"] {
            let err = limits_err(raw);
            assert!(err.contains("is not an option"), "{raw}: {err}");
        }
        assert!(limits_err("claude").starts_with("'claude'"));
    }

    #[test]
    fn agent_limits_values_use_the_wnice_parser() {
        for (raw, want) in [
            ("-c 0", "invalid -c/--cpu value '0'"),
            ("-c 101", "invalid -c/--cpu value '101'"),
            ("-c", "missing value for -c/--cpu"),
            ("-p bogus", "invalid -p/--priority value 'bogus'"),
            ("-t 0", "invalid -t/--threads value '0'"),
            ("-t 9", "invalid -t/--threads value '9'"),
            ("-m 101", "invalid -m/--memory <size>"),
            ("-m 0m", "invalid -m/--memory <size>"),
            ("-n 0", "invalid -n/--procs value '0'"),
            ("-s 0", "invalid -s/--timeout <seconds>"),
            ("-s 0.0009", "invalid -s/--timeout <seconds>"),
            ("--boost", "--boost requires -p/--priority"),
            ("--cpu=50", "unknown option '--cpu=50'"),
            ("-x", "unknown option '-x'"),
        ] {
            assert_eq!(limits_err(raw), want, "{raw}");
        }
    }
}
