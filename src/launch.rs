//! Command-line construction and target-launch decision logic.
//!
//! Ports of the C# `ArgvQuote` / `BuildArgvCommandLine` and the PowerShell
//! `$commandLine` cmd.exe-fallback quoting in `bin/caps.ps1` (same logic as
//! `bin/capc.ps1`), plus the launch-order decision from `CapsLauncher.Run`:
//! `.bat`/`.cmd` targets skip the direct CreateProcess attempt entirely and go
//! straight to the cmd.exe fallback; everything else tries CreateProcess
//! first (bare names are resolved beforehand, against explicit PATH dirs
//! only — never the cwd) and falls back to cmd.exe on failure.

/// Exact port of the C# `ArgvQuote` (standard MSVCRT/CommandLineToArgvW
/// quoting): safe for a directly-launched .exe's own argv parsing. No cmd.exe
/// involved on this path, so none of its operator or `%` expansion semantics
/// apply.
pub fn argv_quote(arg: &str) -> String {
    // Fast path: non-empty and contains none of space, tab, \n, \v, '"'.
    if !arg.is_empty()
        && !arg
            .chars()
            .any(|c| matches!(c, ' ' | '\t' | '\n' | '\u{0B}' | '"'))
    {
        return arg.to_string();
    }
    let mut result = String::new();
    result.push('"');
    let mut backslashes = 0usize;
    for c in arg.chars() {
        if c == '\\' {
            backslashes += 1;
        } else if c == '"' {
            // Backslashes preceding a quote are doubled, plus one more to
            // escape the quote itself.
            for _ in 0..backslashes * 2 + 1 {
                result.push('\\');
            }
            result.push('"');
            backslashes = 0;
        } else {
            if backslashes > 0 {
                for _ in 0..backslashes {
                    result.push('\\');
                }
                backslashes = 0;
            }
            result.push(c);
        }
    }
    // Trailing backslashes are doubled so they don't escape the closing quote.
    if backslashes > 0 {
        for _ in 0..backslashes * 2 {
            result.push('\\');
        }
    }
    result.push('"');
    result
}

/// Join `argv_quote`d parts with spaces (port of the C# `BuildArgvCommandLine`).
pub fn build_argv_command_line(argv: &[&str]) -> String {
    argv.iter()
        .map(|a| argv_quote(a))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Port of the PowerShell `$commandLine` fallback logic in caps.ps1/capc.ps1:
/// escape each arg by replacing `"` with `\"`, then quote the escaped form if
/// it is empty or matches `[\s"&|<>^]`. Quoting neutralizes cmd.exe operators
/// so metacharacters survive as literal argument text.
pub fn build_cmd_fallback_command_line(argv: &[&str]) -> String {
    argv.iter()
        .map(|arg| {
            let escaped = arg.replace('"', "\\\"");
            if escaped.is_empty()
                || escaped.chars().any(|c| {
                    matches!(
                        c,
                        ' ' | '\t' | '\n' | '\u{0B}' | '"' | '&' | '|' | '<' | '>' | '^'
                    )
                })
            {
                format!("\"{escaped}\"")
            } else {
                escaped
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `<cmd_exe_path> /d /v:off /s /c "<fallback>"`, with the extra outer quote
/// pair per cmd's /S rule: cmd strips exactly that pair and leaves the rest
/// untouched. /d skips AutoRun; /v:off disables delayed expansion (`!var!`).
pub fn shell_command_line(cmd_exe_path: &str, fallback: &str) -> String {
    format!("\"{}\" /d /v:off /s /c \"{}\"", cmd_exe_path, fallback)
}

/// True if any argument contains a literal `%` (which could trigger
/// environment-variable expansion inside cmd.exe, even across separate
/// arguments — cmd pairs up `%` characters over the whole command line).
pub fn contains_percent(argv: &[&str]) -> bool {
    argv.iter().any(|a| a.contains('%'))
}

/// `.bat`/`.cmd`, ASCII case-insensitive.
pub fn is_bat_or_cmd(target: &str) -> bool {
    let lower = target.to_ascii_lowercase();
    lower.ends_with(".bat") || lower.ends_with(".cmd")
}

/// True when the `%` refusal applies: the cmd.exe fallback is about to be
/// used (i.e. the direct CreateProcess attempt did NOT create the process)
/// and some argument contains `%`. On the direct path cmd.exe never runs, so
/// `%` is harmless there.
pub fn needs_percent_refusal(argv: &[&str], direct_created: bool) -> bool {
    !direct_created && contains_percent(argv)
}

/// Exact refusal message, matching caps.ps1's InvalidOperationException text.
pub const PERCENT_REFUSAL_MESSAGE: &str = "Refusing to run: argument contains '%' and the target needs the cmd.exe fallback (not a directly-launchable .exe), where '%' can trigger unintended environment-variable expansion. See README's Argument handling section.";

use std::path::Path;

/// Extensions CreateProcess can start: .com/.exe directly, .bat/.cmd via
/// cmd.exe. Also the default search order when PATHEXT is unset.
const LAUNCHABLE_EXTS: [&str; 4] = [".com", ".exe", ".bat", ".cmd"];

/// Split `PATH` into search directories. Only ABSOLUTE entries are kept and
/// none that is the current directory: `""`, `"."`, `".\\"`, relative
/// entries and an absolute entry equal to `cwd` are dropped, so the cwd is
/// never searched.
fn split_path_var(value: &str, cwd: Option<&Path>) -> Vec<String> {
    value
        .split(';')
        .map(|s| s.trim().trim_matches('"'))
        .filter(|s| Path::new(s).is_absolute())
        .filter(|s| match cwd {
            Some(c) => !same_dir(Path::new(s), c),
            None => true,
        })
        .map(str::to_string)
        .collect()
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// The search directories from the process `PATH` (see [`split_path_var`]).
pub fn path_dirs() -> Vec<String> {
    let cwd = std::env::current_dir().ok();
    split_path_var(&std::env::var("PATH").unwrap_or_default(), cwd.as_deref())
}

/// Launchable extensions in PATHEXT order (lowercase, deduplicated). PATHEXT
/// entries CreateProcess cannot start (.JS, .VBS, ...) are ignored; unset or
/// nothing usable gives the default `.com;.exe;.bat;.cmd`.
fn launchable_exts(pathext: Option<&str>) -> Vec<String> {
    let mut exts: Vec<String> = Vec::new();
    for e in pathext.unwrap_or("").split(';') {
        let e = e.trim().to_ascii_lowercase();
        if LAUNCHABLE_EXTS.contains(&e.as_str()) && !exts.contains(&e) {
            exts.push(e);
        }
    }
    if exts.is_empty() {
        exts = LAUNCHABLE_EXTS.iter().map(|e| e.to_string()).collect();
    }
    exts
}

/// Resolve a bare (separator-free) command name against explicit dirs only,
/// with PATHEXT from the environment. See [`resolve_bare_name_with`].
pub fn resolve_bare_name(dirs: &[String], name: &str) -> Option<String> {
    let pathext = std::env::var("PATHEXT").ok();
    resolve_bare_name_with(dirs, &launchable_exts(pathext.as_deref()), name)
}

/// Like cmd.exe: for each dir in order, try `name` as-is (only when it
/// already ends in a launchable extension), then `name` + each of `exts`;
/// the first existing FILE wins. The current directory is NEVER consulted.
pub fn resolve_bare_name_with(dirs: &[String], exts: &[String], name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let mut candidates: Vec<String> = Vec::with_capacity(exts.len() + 1);
    if LAUNCHABLE_EXTS.iter().any(|e| lower.ends_with(e)) {
        candidates.push(name.to_string());
    }
    for ext in exts {
        candidates.push(format!("{name}{ext}"));
    }
    for dir in dirs {
        for cand in &candidates {
            let p = Path::new(dir).join(cand);
            if p.is_file() {
                return Some(p.to_string_lossy().into_owned());
            }
        }
    }
    None
}

/// Outcome of command resolution: `Direct` goes to CreateProcess with
/// lpApplicationName set; `Shell` (a resolved .bat/.cmd) must go through
/// cmd.exe.
#[derive(Debug, PartialEq, Eq)]
pub enum Resolved {
    Direct(String),
    Shell(String),
}

/// Resolve argv[0]. A name containing a path separator is user-explicit and
/// returned as-is (Direct, no search). A bare name is searched in `dirs`
/// only; a hit ending in .bat/.cmd is Shell, everything else Direct.
pub fn resolve_command(dirs: &[String], argv0: &str) -> Option<Resolved> {
    if argv0.contains('/') || argv0.contains('\\') {
        return Some(Resolved::Direct(argv0.to_string()));
    }
    resolve_bare_name(dirs, argv0).map(|abs| {
        if is_bat_or_cmd(&abs) {
            Resolved::Shell(abs)
        } else {
            Resolved::Direct(abs)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_word_unchanged() {
        assert_eq!(argv_quote("hello"), "hello");
        assert_eq!(argv_quote("C:\\foo\\bar.exe"), "C:\\foo\\bar.exe");
    }

    #[test]
    fn arg_with_space_quoted() {
        assert_eq!(argv_quote("hello world"), "\"hello world\"");
    }

    #[test]
    fn arg_with_quote_quoted_and_escaped() {
        assert_eq!(argv_quote("he\"llo"), "\"he\\\"llo\"");
    }

    #[test]
    fn trailing_backslashes_doubled() {
        // "C:\foo\" contains no whitespace/quote -> C# fast path returns it
        // unchanged (exact port).
        assert_eq!(argv_quote("C:\\foo\\"), "C:\\foo\\");
        // Once quoting is required (whitespace present), trailing backslashes
        // are doubled so they don't escape the closing quote.
        assert_eq!(argv_quote("C:\\foo bar\\"), "\"C:\\foo bar\\\\\"");
        assert_eq!(argv_quote("a b\\"), "\"a b\\\\\"");
    }

    #[test]
    fn backslashes_before_quote_doubled_plus_one() {
        assert_eq!(argv_quote("a\\\"b"), "\"a\\\\\\\"b\"");
    }

    #[test]
    fn empty_arg_becomes_empty_quotes() {
        assert_eq!(argv_quote(""), "\"\"");
    }

    #[test]
    fn build_argv_command_line_joins() {
        assert_eq!(build_argv_command_line(&["a b", "c"]), "\"a b\" c");
        assert_eq!(build_argv_command_line(&[]), "");
    }

    #[test]
    fn cmd_fallback_plain_word_unquoted() {
        assert_eq!(build_cmd_fallback_command_line(&["plain"]), "plain");
    }

    #[test]
    fn cmd_fallback_metacharacters_quoted() {
        for arg in ["A&B", "A|B", "A<B>C", "A^B", "with space"] {
            assert_eq!(
                build_cmd_fallback_command_line(&[arg]),
                format!("\"{arg}\"")
            );
        }
        assert_eq!(
            build_cmd_fallback_command_line(&["A&B", "A|B", "A<B>C", "A^B"]),
            "\"A&B\" \"A|B\" \"A<B>C\" \"A^B\""
        );
    }

    #[test]
    fn cmd_fallback_empty_arg_quoted() {
        assert_eq!(build_cmd_fallback_command_line(&[""]), "\"\"");
    }

    #[test]
    fn cmd_fallback_quote_escaped_with_backslash() {
        assert_eq!(build_cmd_fallback_command_line(&["a\"b"]), "\"a\\\"b\"");
    }

    #[test]
    fn cmd_fallback_percent_neutralized_only_by_quoting() {
        // 100%OFF contains no metacharacter chars -> unquoted (direct path
        // never hits cmd, fallback path refuses % before reaching cmd).
        assert_eq!(build_cmd_fallback_command_line(&["100%OFF"]), "100%OFF");
    }

    #[test]
    fn shell_command_line_exact() {
        assert_eq!(
            shell_command_line("C:\\Windows\\System32\\cmd.exe", "foo \"A&B\""),
            "\"C:\\Windows\\System32\\cmd.exe\" /d /v:off /s /c \"foo \"A&B\"\""
        );
    }

    #[test]
    fn contains_percent_basic() {
        assert!(contains_percent(&["100%OFF"]));
        assert!(contains_percent(&["a", "b%c"]));
        assert!(!contains_percent(&["a", "b"]));
        assert!(!contains_percent(&[]));
    }

    #[test]
    fn is_bat_or_cmd_case_insensitive() {
        assert!(is_bat_or_cmd("foo.bat"));
        assert!(is_bat_or_cmd("FOO.BAT"));
        assert!(is_bat_or_cmd("Bar.CMD"));
        assert!(is_bat_or_cmd("x.CmD"));
        assert!(!is_bat_or_cmd("foo.exe"));
        assert!(!is_bat_or_cmd("foo.bat.exe"));
        assert!(!is_bat_or_cmd("bat"));
    }

    #[test]
    fn needs_percent_refusal_matches_direct_path() {
        // Fallback used + % arg -> refuse.
        assert!(needs_percent_refusal(&["100%OFF"], false));
        // Direct CreateProcess succeeded -> cmd never runs, % is harmless.
        assert!(!needs_percent_refusal(&["100%OFF"], true));
        // No % anywhere -> never refuse.
        assert!(!needs_percent_refusal(&["plain"], false));
    }

    // ---- path splitting / resolution ----

    static PROBE_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    /// Create a unique temp dir containing the given files; returns the dir.
    fn probe_dir(files: &[&str]) -> std::path::PathBuf {
        use std::sync::atomic::Ordering;
        let n = PROBE_SEQ.fetch_add(1, Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("wnice-launch-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        for f in files {
            std::fs::write(d.join(f), b"").unwrap();
        }
        d
    }

    #[test]
    fn split_path_var_keeps_only_absolute_non_cwd_entries() {
        assert_eq!(split_path_var("", None), Vec::<String>::new());
        assert_eq!(
            split_path_var(r";;.;.\;bin;..\x;C:rel", None),
            Vec::<String>::new()
        );
        assert_eq!(
            split_path_var(r#"C:\a;;  .  ;"C:\b""#, None),
            vec![r"C:\a".to_string(), r"C:\b".to_string()]
        );
        let cwd = probe_dir(&[]);
        let cwd_s = cwd.to_string_lossy().into_owned();
        assert_eq!(
            split_path_var(&format!(r"{cwd_s};C:\a;{cwd_s}\"), Some(&cwd)),
            vec![r"C:\a".to_string()]
        );
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn cwd_executable_loses_to_path_executable() {
        // Same name in the cwd and on PATH, cwd listed FIRST on PATH (as "."
        // and as its absolute path): the PATH copy must win.
        let cwd = probe_dir(&["zzwnice-hijack.exe"]);
        let good = probe_dir(&["zzwnice-hijack.exe"]);
        let path = format!(r".;.\;{};{}", cwd.to_string_lossy(), good.to_string_lossy());
        let dirs = split_path_var(&path, Some(&cwd));
        assert_eq!(
            resolve_bare_name_with(&dirs, &launchable_exts(None), "zzwnice-hijack"),
            Some(
                good.join("zzwnice-hijack.exe")
                    .to_string_lossy()
                    .into_owned()
            )
        );
        let _ = std::fs::remove_dir_all(&cwd);
        let _ = std::fs::remove_dir_all(&good);
    }

    #[test]
    fn launchable_exts_follow_pathext_order() {
        assert_eq!(launchable_exts(None), vec![".com", ".exe", ".bat", ".cmd"]);
        assert_eq!(
            launchable_exts(Some(".JS;.CMD;.EXE;.cmd;.VBS")),
            vec![".cmd", ".exe"]
        );
        assert_eq!(
            launchable_exts(Some(".JS")),
            vec![".com", ".exe", ".bat", ".cmd"]
        );
        // PATHEXT order decides between candidates in the same dir.
        let d = probe_dir(&["zzwnice-order.exe", "zzwnice-order.cmd"]);
        let dirs = vec![d.to_string_lossy().into_owned()];
        assert_eq!(
            resolve_bare_name_with(&dirs, &launchable_exts(Some(".CMD;.EXE")), "zzwnice-order"),
            Some(d.join("zzwnice-order.cmd").to_string_lossy().into_owned())
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn resolve_bare_name_never_falls_back_to_cwd_or_search() {
        // A deliberately nonexistent dir: nothing may be found, not even
        // "cmd", and the cwd is never consulted implicitly.
        let dirs = vec!["C:\\__definitely_nonexistent_dir__".to_string()];
        assert_eq!(resolve_bare_name(&dirs, "cmd"), None);
        assert_eq!(resolve_bare_name(&dirs, "cmd.exe"), None);
    }

    #[test]
    fn resolve_bare_name_exact_join_and_ext_order() {
        let d = probe_dir(&["zzwnice-test-probe.exe"]);
        let dirs = vec![d.to_string_lossy().into_owned()];
        // Bare name gets .exe appended; result is dir\candidate exactly.
        assert_eq!(
            resolve_bare_name(&dirs, "zzwnice-test-probe"),
            Some(
                d.join("zzwnice-test-probe.exe")
                    .to_string_lossy()
                    .into_owned()
            )
        );
        // Suffix matching is ASCII case-insensitive (.EXE as-is candidate).
        assert_eq!(
            resolve_bare_name(&dirs, "zzwnice-test-probe.EXE"),
            Some(
                d.join("zzwnice-test-probe.EXE")
                    .to_string_lossy()
                    .into_owned()
            )
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn resolve_command_separator_is_direct_without_search() {
        // User-explicit path: returned verbatim even if it does not exist.
        assert_eq!(
            resolve_command(&[], ".\\nope\\missing.exe"),
            Some(Resolved::Direct(".\\nope\\missing.exe".to_string()))
        );
        assert_eq!(
            resolve_command(&[], "C:/x/y.exe"),
            Some(Resolved::Direct("C:/x/y.exe".to_string()))
        );
    }

    #[test]
    fn resolve_command_bat_hit_is_shell() {
        let d = probe_dir(&["zzwnice-test-probe.bat"]);
        let dirs = vec![d.to_string_lossy().into_owned()];
        assert_eq!(
            resolve_command(&dirs, "zzwnice-test-probe"),
            Some(Resolved::Shell(
                d.join("zzwnice-test-probe.bat")
                    .to_string_lossy()
                    .into_owned()
            ))
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn resolve_command_bare_no_hit_is_none() {
        let d = probe_dir(&[]);
        let dirs = vec![d.to_string_lossy().into_owned()];
        assert_eq!(resolve_command(&dirs, "zzwnice-no-such-tool"), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
