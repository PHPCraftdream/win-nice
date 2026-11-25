//! Where a `--jobs` token counts as a wnice option.

/// Whether a `--jobs` token sits in the options part of `args`: before the
/// first `--` and before the first non-option token, with the value of a
/// value-taking option read as that value and never as a flag.
pub(super) fn among_options(args: &[String]) -> bool {
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        match arg.as_str() {
            "--" => return false,
            "--jobs" => return true,
            "-p" | "--priority" | "-c" | "--cpu" | "-t" | "--threads" | "-m" | "--memory"
            | "-n" | "--procs" | "-s" | "--timeout" | "--adopt" => i += 2,
            a if a.starts_with('-') && a.len() > 1 => i += 1,
            _ => return false,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::among_options;

    #[test]
    fn only_the_options_part_is_scanned() {
        let among = |v: &[&str]| {
            let owned: Vec<String> = v.iter().map(|a| a.to_string()).collect();
            among_options(&owned)
        };
        assert!(among(&["--jobs"]));
        assert!(among(&["-V", "--jobs"]));
        assert!(among(&["-c", "10", "--jobs", "cmd"]));
        assert!(among(&["--wsl=x", "--boost", "--jobs"]));
        assert!(!among(&[]));
        assert!(!among(&["cmd", "--jobs"]));
        assert!(!among(&["-c", "10", "cmd", "--jobs"]));
        assert!(!among(&["--", "--jobs"]));
        assert!(!among(&["-p", "--jobs"]));
        assert!(!among(&["-c"]));
        assert!(!among(&["-", "--jobs"]));
    }
}
