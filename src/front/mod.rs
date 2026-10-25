//! Single-purpose frontends selected by argv[0] (the hardlinks postinstall
//! creates next to wnice.exe).

pub mod admin;
pub mod help;
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

/// Run the frontend named `stem`; `None` when `stem` is not one.
pub fn dispatch(stem: &str, args: &[String]) -> Option<i32> {
    let sys = RealSys;
    if help::wants_help(args) {
        if let Some(text) = help::text(stem) {
            println!("{text}");
            return Some(0);
        }
    }
    if legacy::is_legacy(stem) {
        return Some(
            match legacy::parse(stem, args, sys.logical_processor_count()) {
                Ok(cfg) => crate::run::run(&cfg, &sys),
                Err(usage) => {
                    eprintln!("{usage}");
                    eprintln!("{}", help::hint(stem));
                    1
                }
            },
        );
    }
    if let Some(prefix) = agent_prefix(stem) {
        let mut command: Vec<String> = prefix.iter().map(|s| s.to_string()).collect();
        command.extend_from_slice(args);
        let cfg = Config {
            command,
            ..Config::default()
        };
        return Some(crate::run::run(&cfg, &sys));
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
}
