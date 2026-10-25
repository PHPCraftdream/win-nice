//! `admin <command> [args...]`: run a command elevated. Already elevated →
//! inline (console, exit code kept); otherwise the UAC prompt via
//! ShellExecuteEx "runas" in its own window, and its exit code.

use super::native;
use crate::launch::{
    build_argv_command_line, build_cmd_fallback_command_line, contains_percent, is_bat_or_cmd,
    path_dirs, resolve_command, Resolved,
};
use crate::{Config, RealSys, Sys};

pub const USAGE: &str = "usage: admin <command> [args...]";
pub const PERCENT_REFUSAL: &str = "Refusing to run: argument contains '%', which cmd.exe could \
expand as an environment variable during elevation. See README's Argument handling section.";

/// How a not-yet-elevated `admin` launches its target.
#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    /// A real executable: elevated directly by absolute path, no cmd.exe.
    Direct { file: String, params: String },
    /// .bat/.cmd, cmd.exe builtins and unresolvable names: through cmd.exe.
    Shell { params: String },
    /// Shell route with a `%` in some argument (cmd.exe would expand it).
    RefusePercent,
}

pub fn route(dirs: &[String], args: &[String]) -> Route {
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    if !is_bat_or_cmd(argv[0]) {
        if let Some(Resolved::Direct(p)) = resolve_command(dirs, argv[0]) {
            let file = std::path::absolute(&p)
                .map(|a| a.to_string_lossy().into_owned())
                .unwrap_or(p);
            return Route::Direct {
                file,
                params: build_argv_command_line(&argv[1..]),
            };
        }
    }
    if contains_percent(&argv) {
        return Route::RefusePercent;
    }
    Route::Shell {
        params: format!(
            "/d /s /v:off /c \"{}\"",
            build_cmd_fallback_command_line(&argv)
        ),
    }
}

pub fn main(args: &[String]) -> i32 {
    if args.is_empty() {
        eprintln!("{USAGE}");
        eprintln!("{}", super::help::hint("admin"));
        return 1;
    }
    let sys = RealSys;
    if native::is_elevated() {
        let cfg = Config {
            command: args.to_vec(),
            ..Config::default()
        };
        return crate::run::run(&cfg, &sys);
    }
    let (file, params) = match route(&path_dirs(), args) {
        Route::Direct { file, params } => (file, params),
        Route::Shell { params } => (format!("{}\\cmd.exe", sys.system_directory()), params),
        Route::RefusePercent => {
            eprintln!("{PERCENT_REFUSAL}");
            return 1;
        }
    };
    match native::run_elevated(&file, &params) {
        Ok(code) => code as i32,
        Err(code) => {
            eprintln!(
                "Elevation was cancelled or failed: {}",
                native::os_message(code)
            );
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn sys32() -> Vec<String> {
        vec![RealSys.system_directory()]
    }

    #[test]
    fn real_exe_routes_direct_by_absolute_path_with_argv_quoting() {
        match route(&sys32(), &a(&["cmd", "/c", "echo a b"])) {
            Route::Direct { file, params } => {
                assert!(std::path::Path::new(&file).is_absolute(), "{file}");
                assert!(file.to_ascii_lowercase().ends_with("cmd.exe"), "{file}");
                assert_eq!(params, "/c \"echo a b\"");
            }
            other => panic!("expected Direct, got {other:?}"),
        }
    }

    #[test]
    fn percent_is_harmless_on_the_direct_route() {
        assert!(matches!(
            route(&sys32(), &a(&["cmd", "/c", "echo", "100%"])),
            Route::Direct { .. }
        ));
    }

    #[test]
    fn bat_builtin_and_unknown_names_go_through_cmd() {
        for target in ["build.bat", "x.CMD", "ver", "zz-no-such-tool"] {
            match route(&sys32(), &a(&[target, "a b"])) {
                Route::Shell { params } => {
                    assert!(params.starts_with("/d /s /v:off /c \""), "{params}");
                    assert!(params.contains("\"a b\""), "{params}");
                }
                other => panic!("{target}: expected Shell, got {other:?}"),
            }
        }
    }

    #[test]
    fn percent_on_the_shell_route_is_refused() {
        assert_eq!(
            route(&sys32(), &a(&["somebatch.bat", "100%OFF"])),
            Route::RefusePercent
        );
    }
}
