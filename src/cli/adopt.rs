//! The value of `--adopt`: a comma-separated list of image names.

/// `--adopt` with `-n`: joining a process past the active-process limit
/// terminates it (AssignProcessToJobObject), so wnice would kill the very
/// servers it was asked to cap.
pub(super) const PROCS_CONFLICT: &str = "--adopt cannot be combined with -n/--procs: a process that would exceed the active-process limit is terminated when it joins the job";

/// Normalized image names: lower case, always with the `.exe` suffix (`NAME`
/// and `NAME.exe` are the same), duplicates dropped. No paths, wildcards or
/// empty elements, no element starting with `-`; at most 16 names of at most
/// 64 characters.
pub(super) fn parse_adopt_names(value: &str) -> Result<Vec<String>, String> {
    let invalid = || "invalid --adopt value".to_string();
    let parts: Vec<&str> = value.split(',').collect();
    if parts.len() > 16 {
        return Err(invalid());
    }
    let mut names = Vec::with_capacity(parts.len());
    for part in parts {
        // A leading '-' is a forgotten value (`--adopt -- cmd`), not a name.
        if part.is_empty()
            || part.starts_with('-')
            || part.chars().count() > 64
            || part
                .chars()
                .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?'))
        {
            return Err(invalid());
        }
        let lower = part.to_lowercase();
        let name = if lower.ends_with(".exe") {
            lower
        } else {
            format!("{lower}.exe")
        };
        if !names.iter().any(|existing| existing == &name) {
            names.push(name);
        }
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::{parse_adopt_names, PROCS_CONFLICT};
    use crate::cli::{parse_args, ParseError};

    fn parse(v: &[&str]) -> Result<crate::cli::Action, ParseError> {
        let args: Vec<String> = v.iter().map(|s| s.to_string()).collect();
        parse_args(&args, 8)
    }

    #[test]
    fn adopt_with_procs_is_a_conflict_in_either_order() {
        let want = Err(ParseError::Conflict(PROCS_CONFLICT.to_string()));
        for line in [
            &["--adopt", "node", "-n", "4", "--", "cmd"][..],
            &["-n", "4", "--adopt", "node", "cmd"],
            &["--procs", "1000", "-c", "20", "--adopt", "a,b", "cmd"],
            &["--adopt", "node", "-n", "4"],
        ] {
            assert_eq!(parse(line), want, "{line:?}");
        }
        assert!(!PROCS_CONFLICT.contains('\n'));
    }

    #[test]
    fn adopt_and_procs_stay_valid_apart_and_with_other_limits() {
        for line in [
            &["--adopt", "node", "-m", "50", "-c", "20", "cmd"][..],
            &["-n", "4", "cmd"],
            // after the command's first word they are the command's own arguments
            &["--adopt", "node", "cmd", "-n", "4"],
        ] {
            assert!(
                matches!(parse(line), Ok(crate::cli::Action::Run(_))),
                "{line:?}"
            );
        }
    }

    #[test]
    fn wsl_conflict_wins_over_the_procs_conflict() {
        assert_eq!(
            parse(&["--adopt", "node", "--wsl", "-n", "4", "cmd"]),
            Err(ParseError::Conflict(
                "--adopt cannot be combined with --wsl".to_string()
            ))
        );
    }

    #[test]
    fn names_are_normalized() {
        assert_eq!(
            parse_adopt_names("Microsoft Edge,Tool Name.exe").unwrap(),
            vec!["microsoft edge.exe", "tool name.exe"]
        );
        assert_eq!(
            parse_adopt_names("MSBuild,msbuild.EXE,node").unwrap(),
            vec!["msbuild.exe", "node.exe"]
        );
    }

    #[test]
    fn invalid_lists_are_refused() {
        let long = "a".repeat(65);
        let many = "a,b,c,d,e,f,g,h,i,j,k,l,m,n,o,p,q";
        for value in [
            "",
            ",node",
            "node,",
            "node,,tool",
            "../x",
            "a\\b",
            "*",
            "?",
            "name:stream.exe",
            "--",
            "-p",
            "node,-x",
            long.as_str(),
            many,
        ] {
            assert_eq!(
                parse_adopt_names(value),
                Err("invalid --adopt value".into()),
                "{value}"
            );
        }
    }
}
