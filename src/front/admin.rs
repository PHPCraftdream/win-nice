//! `admin <command> [args...]`: run a command elevated. Already elevated →
//! inline (console, exit code kept); otherwise the UAC prompt via
//! ShellExecuteEx "runas" in its own window, and its exit code.
//!
//! `admin --keep-limits <command> [args...]` elevates this executable's
//! `wnice` front with the limits of the caller's job instead of the command,
//! so a UAC-elevated child keeps them (a separate new job, not the caller's).

use std::path::Path;

use super::native;
use crate::launch::{
    build_argv_command_line, build_cmd_fallback_command_line, contains_percent, is_bat_or_cmd,
    path_dirs, resolve_command, Resolved,
};
use crate::{Config, PriorityClass, RealSys, Sys};

pub const USAGE: &str = "usage: admin <command> [args...]";
pub const PERCENT_REFUSAL: &str = "Refusing to run: argument contains '%', which cmd.exe could \
expand as an environment variable during elevation. See README's Argument handling section.";
pub const KEEP_LIMITS: &str = "--keep-limits";
pub const NOTHING_TO_KEEP: &str =
    "admin: --keep-limits: the caller's job has no limits to keep; running as plain admin.";

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

/// The limits of the job this process belongs to, as the kernel reports them
/// (`QueryInformationJobObject` with a NULL handle).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallerLimits {
    pub flags: u32,
    pub affinity: usize,
    pub active_process_limit: u32,
    pub priority_class: u32,
    pub job_memory_limit: u64,
    pub cpu_control_flags: u32,
    /// `CpuRate` as the kernel stores it: percent * 100.
    pub cpu_rate: u32,
}

/// What the caller's immediate job reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallerJob {
    /// The process belongs to no job.
    NoJob,
    /// A query failed; `code` is the Win32 error of `query`.
    Failed { query: &'static str, code: u32 },
    /// The job's limits.
    Limits(CallerLimits),
}

/// What `--keep-limits` can do with the caller's job: the `wnice` options that
/// reproduce the preserved limits, one stderr note per limit that cannot be
/// kept, and whether the job carries any limit at all.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub options: Vec<String>,
    pub notes: Vec<String>,
    /// The caller's job carries at least one limit `--keep-limits` knows.
    pub carried: bool,
}

const PRIORITIES: [PriorityClass; 6] = [
    PriorityClass::Idle,
    PriorityClass::BelowNormal,
    PriorityClass::Normal,
    PriorityClass::AboveNormal,
    PriorityClass::High,
    PriorityClass::Realtime,
];

fn priority_name(p: PriorityClass) -> &'static str {
    match p {
        PriorityClass::Idle => "idle",
        PriorityClass::BelowNormal => "belownormal",
        PriorityClass::Normal => "normal",
        PriorityClass::AboveNormal => "abovenormal",
        PriorityClass::High => "high",
        PriorityClass::Realtime => "realtime",
    }
}

/// `n` when `mask` is `n` contiguous bits from CPU 0, else `None`.
fn threads_of(mask: usize) -> Option<u32> {
    let n = mask.count_ones();
    (n > 0 && n < usize::BITS && mask == (1usize << n) - 1).then_some(n)
}

/// `-m` value for an absolute byte count: `<n>m` for whole MiB, else the exact
/// decimal fraction of a MiB (`rem * 5^20` is `rem / 2^20` in 20 digits, which
/// the parser floors back to the same byte count).
fn memory_arg(bytes: u64) -> String {
    const MIB: u64 = 1 << 20;
    let (whole, rem) = (bytes / MIB, bytes % MIB);
    if rem == 0 {
        return format!("{whole}m");
    }
    let frac = format!("{:020}", u128::from(rem) * 5u128.pow(20));
    format!("{whole}.{}m", frac.trim_end_matches('0'))
}

/// The `wnice` options that reproduce the preserved limits: `-c`, `-m`, `-n`,
/// `-t`, `-p` in that order. `logical_cpus` bounds `-t` like the parser does.
pub fn plan(l: &CallerLimits, logical_cpus: u32) -> Plan {
    let mut out = Plan::default();
    if l.cpu_control_flags & crate::JOB_OBJECT_CPU_RATE_CONTROL_ENABLE != 0 {
        out.carried = true;
        if l.cpu_control_flags & crate::JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP == 0 {
            out.notes.push(format!(
                "admin: --keep-limits: the caller's CPU rate control is not a hard cap (control \
                 flags {:#x}); it cannot be kept",
                l.cpu_control_flags
            ));
        } else {
            let percent = l.cpu_rate / 100;
            if percent == 0 {
                out.options.extend(["-c".to_string(), "1".to_string()]);
                out.notes.push(format!(
                    "admin: --keep-limits: the caller's CPU rate cap {}.{:02}% is below 1%, the \
                     smallest whole percent wnice -c accepts; keeping -c 1",
                    percent,
                    l.cpu_rate % 100
                ));
            } else {
                if !l.cpu_rate.is_multiple_of(100) {
                    out.notes.push(format!(
                        "admin: --keep-limits: the caller's CPU rate cap {}.{:02}% was rounded \
                         down to -c {percent}; wnice -c takes whole percents",
                        percent,
                        l.cpu_rate % 100
                    ));
                }
                out.options.extend(["-c".to_string(), percent.to_string()]);
            }
        }
    }
    if l.flags & crate::JOB_OBJECT_LIMIT_JOB_MEMORY != 0 {
        out.carried = true;
        if l.job_memory_limit > 0 {
            out.options
                .extend(["-m".to_string(), memory_arg(l.job_memory_limit)]);
        } else {
            out.notes.push(
                "admin: --keep-limits: the job memory limit is 0 bytes; it cannot be kept"
                    .to_string(),
            );
        }
    }
    if l.flags & crate::JOB_OBJECT_LIMIT_ACTIVE_PROCESS != 0 {
        out.carried = true;
        if l.active_process_limit > 0 {
            out.options
                .extend(["-n".to_string(), l.active_process_limit.to_string()]);
        } else {
            out.notes.push(
                "admin: --keep-limits: the active-process limit is 0; it cannot be kept"
                    .to_string(),
            );
        }
    }
    if l.flags & crate::JOB_OBJECT_LIMIT_AFFINITY != 0 {
        out.carried = true;
        match threads_of(l.affinity).filter(|&n| n <= logical_cpus.clamp(1, 63)) {
            Some(n) => out.options.extend(["-t".to_string(), n.to_string()]),
            None => out.notes.push(format!(
                "admin: --keep-limits: the job affinity could not be kept (mask {:#x} is not a \
                 contiguous run from CPU 0); elevating without it.",
                l.affinity
            )),
        }
    }
    if l.flags & crate::JOB_OBJECT_LIMIT_PRIORITY_CLASS != 0 {
        out.carried = true;
        match PRIORITIES
            .iter()
            .copied()
            .find(|p| p.to_u32() == l.priority_class)
        {
            Some(p) => out
                .options
                .extend(["-p".to_string(), priority_name(p).to_string()]),
            None => out.notes.push(format!(
                "admin: --keep-limits: the priority class could not be kept ({:#x}).",
                l.priority_class
            )),
        }
    }
    out
}

/// The ShellExecuteEx parameter string: `<options> -- <command> [args...]`,
/// argv-quoted like the direct route.
pub fn elevated_params(options: &[String], command: &[String]) -> String {
    let mut argv: Vec<&str> = options.iter().map(String::as_str).collect();
    argv.push("--");
    argv.extend(command.iter().map(String::as_str));
    build_argv_command_line(&argv)
}

/// This executable's `wnice` front: the same file under another hardlink name
/// (dispatch reads argv[0]) or the installer's byte-identical copy. Anything
/// else is refused before the UAC prompt.
fn wnice_front() -> Result<String, String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("admin: --keep-limits: cannot locate this executable: {e}"))?;
    let front = exe.with_file_name("wnice.exe");
    if !same_front(&exe, &front) {
        return Err(format!(
            "admin: --keep-limits: {} is not this executable's wnice front; refusing to elevate",
            front.display()
        ));
    }
    Ok(front.to_string_lossy().into_owned())
}

/// Same content, so the same installed front: the installer's hardlink and its
/// fallback copy both qualify, a foreign `wnice.exe` of another build does not.
/// (A file's volume serial and file index are still unstable in std.)
fn same_front(exe: &Path, front: &Path) -> bool {
    let (Ok(meta_a), Ok(meta_b)) = (std::fs::metadata(exe), std::fs::metadata(front)) else {
        return false;
    };
    if meta_a.len() != meta_b.len() {
        return false;
    }
    match (std::fs::read(exe), std::fs::read(front)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Start `file` elevated and return its exit code; a cancelled prompt is the
/// existing message and exit 1.
fn elevate(file: &str, params: &str) -> i32 {
    match native::run_elevated(file, params) {
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

/// What `--keep-limits` does next.
#[derive(Debug, PartialEq, Eq)]
pub enum Elevation {
    /// Nothing to keep or nothing that could be kept: use the plain route.
    Plain,
    /// Fail closed before the UAC prompt.
    Fail,
    /// Elevate `params` with `file`.
    Keep { file: String, params: String },
}

/// Split a leading `--keep-limits` off `args`; anywhere else it is a command
/// token.
fn split_keep_limits(args: &[String]) -> (bool, &[String]) {
    match args.split_first() {
        Some((first, rest)) if first == KEEP_LIMITS => (true, rest),
        _ => (false, args),
    }
}

/// Decide the `--keep-limits` step and report every problem through `err`: one
/// line per limit that cannot be kept, one line when there is nothing to keep,
/// one line when the limits cannot be read or the front is not ours.
fn decide(
    job: CallerJob,
    logical_cpus: u32,
    args: &[String],
    front: &mut dyn FnMut() -> Result<String, String>,
    err: &mut dyn FnMut(&str),
) -> Elevation {
    let limits = match job {
        CallerJob::NoJob => {
            err(NOTHING_TO_KEEP);
            return Elevation::Plain;
        }
        CallerJob::Failed { query, code } => {
            err(&format!(
                "admin: --keep-limits: {query} failed: {code}; refusing to elevate without the \
                 caller's limits"
            ));
            return Elevation::Fail;
        }
        CallerJob::Limits(l) => l,
    };
    let plan = plan(&limits, logical_cpus);
    for note in &plan.notes {
        err(note);
    }
    if plan.options.is_empty() {
        if !plan.carried {
            err(NOTHING_TO_KEEP);
        }
        return Elevation::Plain;
    }
    match front() {
        Ok(file) => Elevation::Keep {
            file,
            params: elevated_params(&plan.options, args),
        },
        Err(msg) => {
            err(&msg);
            Elevation::Fail
        }
    }
}

pub fn main(args: &[String]) -> i32 {
    let (keep, args) = split_keep_limits(args);
    if args.is_empty() {
        eprintln!("{USAGE}");
        eprintln!("{}", super::help::hint("admin"));
        return 1;
    }
    let sys = std::sync::Arc::new(RealSys);
    if native::is_elevated() {
        let cfg = Config {
            command: args.to_vec(),
            ..Config::default()
        };
        return crate::run::run(&cfg, sys);
    }
    if keep {
        let mut err = |msg: &str| eprintln!("{msg}");
        match decide(
            native::caller_job(),
            sys.logical_processor_count(),
            args,
            &mut wnice_front,
            &mut err,
        ) {
            Elevation::Plain => {}
            Elevation::Fail => return 1,
            Elevation::Keep { file, params } => return elevate(&file, &params),
        }
    }
    let (file, params) = match route(&path_dirs(), args) {
        Route::Direct { file, params } => (file, params),
        Route::Shell { params } => (format!("{}\\cmd.exe", sys.system_directory()), params),
        Route::RefusePercent => {
            eprintln!("{PERCENT_REFUSAL}");
            return 1;
        }
    };
    elevate(&file, &params)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemSpec;

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

    /// A caller job with every preserved limit: 100% CPU, 512 MiB, 9
    /// processes, 4 CPUs from 0, HIGH priority.
    fn full() -> CallerLimits {
        CallerLimits {
            flags: crate::JOB_OBJECT_LIMIT_AFFINITY
                | crate::JOB_OBJECT_LIMIT_JOB_MEMORY
                | crate::JOB_OBJECT_LIMIT_ACTIVE_PROCESS
                | crate::JOB_OBJECT_LIMIT_PRIORITY_CLASS,
            affinity: 0b1111,
            active_process_limit: 9,
            priority_class: PriorityClass::High.to_u32(),
            job_memory_limit: 512 * 1024 * 1024,
            cpu_control_flags: crate::JOB_OBJECT_CPU_RATE_CONTROL_ENABLE
                | crate::JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
            cpu_rate: 10_000,
        }
    }

    fn empty() -> CallerLimits {
        CallerLimits {
            flags: 0,
            affinity: 0,
            active_process_limit: 0,
            priority_class: 0,
            job_memory_limit: 0,
            cpu_control_flags: 0,
            cpu_rate: 0,
        }
    }

    #[test]
    fn every_preserved_limit_becomes_an_option() {
        let p = plan(&full(), 8);
        assert_eq!(
            p.options,
            a(&["-c", "100", "-m", "512m", "-n", "9", "-t", "4", "-p", "high"])
        );
        assert!(p.notes.is_empty(), "{:?}", p.notes);
        assert!(p.carried);
    }

    #[test]
    fn nothing_to_keep_plans_nothing() {
        assert_eq!(plan(&empty(), 8), Plan::default());
    }

    #[test]
    fn affinity_is_kept_only_as_a_contiguous_run_from_cpu_zero() {
        let mut l = empty();
        l.flags = crate::JOB_OBJECT_LIMIT_AFFINITY;
        for (mask, want) in [
            (1usize, Some("1")),
            (0b111, Some("3")),
            (0b1111, Some("4")),
            (0b101, None),
            (0b1111_0000, None),
            (0, None),
        ] {
            l.affinity = mask;
            let p = plan(&l, 8);
            match want {
                Some(n) => {
                    assert_eq!(p.options, a(&["-t", n]), "mask {mask:#b}");
                    assert!(p.notes.is_empty(), "mask {mask:#b}");
                }
                None => {
                    assert!(p.options.is_empty(), "mask {mask:#b}");
                    assert_eq!(p.notes.len(), 1, "mask {mask:#b}");
                    assert!(
                        p.notes[0].contains("affinity could not be kept"),
                        "mask {mask:#b}: {}",
                        p.notes[0]
                    );
                }
            }
        }
    }

    #[test]
    fn affinity_wider_than_the_machine_is_not_kept() {
        let mut l = empty();
        l.flags = crate::JOB_OBJECT_LIMIT_AFFINITY;
        l.affinity = 0b1111;
        let p = plan(&l, 2);
        assert!(p.options.is_empty());
        assert_eq!(p.notes.len(), 1);
    }

    #[test]
    fn memory_argument_round_trips_through_the_wntice_parser() {
        for bytes in [
            1u64,
            1023,
            1 << 20,
            (1 << 20) + 7,
            512 * 1024 * 1024,
            3 * 1024 * 1024 + 123_457,
            u64::MAX,
        ] {
            let argv = a(&["-m", &memory_arg(bytes), "cmd"]);
            match crate::cli::parse_args(&argv, 8).ok().unwrap() {
                crate::cli::Action::Run(cfg) => {
                    assert_eq!(cfg.memory, Some(MemSpec::Bytes(bytes)), "{bytes}");
                }
                other => panic!("{bytes}: {other:?}"),
            }
        }
    }

    #[test]
    fn options_round_trip_through_the_real_parser() {
        let mut argv = plan(&full(), 8).options;
        argv.extend(a(&["--", "cmd"]));
        match crate::cli::parse_args(&argv, 8).ok().unwrap() {
            crate::cli::Action::Run(cfg) => {
                assert_eq!(cfg.cpu_percent, Some(100));
                assert_eq!(cfg.memory, Some(MemSpec::Bytes(512 * 1024 * 1024)));
                assert_eq!(cfg.procs, Some(9));
                assert_eq!(cfg.threads, Some(4));
                assert_eq!(cfg.priority, Some(PriorityClass::High));
                assert_eq!(cfg.command, vec!["cmd"]);
                assert_eq!(cfg.timeout_ms, None, "the wall-clock timeout is never kept");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn elevated_params_puts_options_first_and_quotes_the_command() {
        assert_eq!(
            elevated_params(&a(&["-c", "10"]), &a(&["cmd", "/c", "echo a b"])),
            "-c 10 -- cmd /c \"echo a b\""
        );
        assert_eq!(
            elevated_params(&a(&["-n", "2"]), &a(&["say\"hi", "-x"])),
            "-n 2 -- \"say\\\"hi\" -x"
        );
        assert_eq!(
            elevated_params(&[], &a(&["-x"])),
            "-- -x",
            "a command that looks like an option still lands after --"
        );
    }

    #[test]
    fn hard_cap_with_a_fractional_percent_is_rounded_down() {
        let mut l = empty();
        l.cpu_control_flags =
            crate::JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | crate::JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP;
        l.cpu_rate = 1050;
        let p = plan(&l, 8);
        assert_eq!(p.options, a(&["-c", "10"]));
        assert_eq!(p.notes.len(), 1);
        assert!(p.notes[0].contains("10.50%"), "{}", p.notes[0]);
        assert!(p.notes[0].contains("-c 10"), "{}", p.notes[0]);
        l.cpu_rate = 1000;
        assert!(
            plan(&l, 8).notes.is_empty(),
            "a whole percent is not rounded"
        );
        l.cpu_rate = 50;
        let p = plan(&l, 8);
        assert_eq!(p.options, a(&["-c", "1"]));
        assert_eq!(p.notes.len(), 1);
        assert!(p.notes[0].contains("below 1%"), "{}", p.notes[0]);
    }

    #[test]
    fn rate_control_that_is_not_a_hard_cap_is_not_kept() {
        let mut l = empty();
        l.cpu_control_flags = crate::JOB_OBJECT_CPU_RATE_CONTROL_ENABLE;
        l.cpu_rate = 1000;
        let p = plan(&l, 8);
        assert!(p.options.is_empty());
        assert!(p.carried);
        assert_eq!(p.notes.len(), 1);
        assert!(p.notes[0].contains("not a hard cap"), "{}", p.notes[0]);
    }

    #[test]
    fn affinity_only_job_reports_the_loss_on_stderr() {
        let mut lines: Vec<String> = Vec::new();
        let mut front_called = false;
        let mut front = || {
            front_called = true;
            Ok("unused".to_string())
        };
        let mut err = |m: &str| lines.push(m.to_string());
        let out = decide(
            CallerJob::Limits(CallerLimits {
                flags: crate::JOB_OBJECT_LIMIT_AFFINITY,
                affinity: 0b101,
                ..empty()
            }),
            8,
            &a(&["cmd"]),
            &mut front,
            &mut err,
        );
        assert_eq!(out, Elevation::Plain);
        assert!(!front_called);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].contains("affinity could not be kept"),
            "{}",
            lines[0]
        );
        assert!(!lines.contains(&NOTHING_TO_KEEP.to_string()), "{lines:?}");
    }

    #[test]
    fn query_failure_fails_closed_before_elevation() {
        let mut lines: Vec<String> = Vec::new();
        let mut front_called = false;
        let mut front = || {
            front_called = true;
            Ok("unused".to_string())
        };
        let mut err = |m: &str| lines.push(m.to_string());
        let out = decide(
            CallerJob::Failed {
                query: "QueryInformationJobObject(JobObjectCpuRateControlInformation)",
                code: 5,
            },
            8,
            &a(&["cmd"]),
            &mut front,
            &mut err,
        );
        assert_eq!(out, Elevation::Fail);
        assert!(!front_called);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].contains(
                "QueryInformationJobObject(JobObjectCpuRateControlInformation) failed: 5"
            ),
            "{}",
            lines[0]
        );
        assert!(lines[0].contains("refusing to elevate"), "{}", lines[0]);
    }

    #[test]
    fn front_that_is_not_this_executable_fails_closed() {
        let mut lines: Vec<String> = Vec::new();
        let mut front = || {
            Err(
                "admin: --keep-limits: C:\\x\\wnice.exe is not this executable's wnice front; \
                 refusing to elevate"
                    .to_string(),
            )
        };
        let mut err = |m: &str| lines.push(m.to_string());
        let out = decide(
            CallerJob::Limits(full()),
            8,
            &a(&["cmd"]),
            &mut front,
            &mut err,
        );
        assert_eq!(out, Elevation::Fail);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].contains("not this executable's wnice front"),
            "{}",
            lines[0]
        );
    }

    #[test]
    fn keep_route_elevates_the_front_with_the_limits() {
        let mut lines: Vec<String> = Vec::new();
        let mut front = || Ok("C:\\tools\\wnice.exe".to_string());
        let mut err = |m: &str| lines.push(m.to_string());
        let out = decide(
            CallerJob::Limits(full()),
            8,
            &a(&["cmd", "/c", "x y"]),
            &mut front,
            &mut err,
        );
        match out {
            Elevation::Keep { file, params } => {
                assert_eq!(file, "C:\\tools\\wnice.exe");
                assert_eq!(params, "-c 100 -m 512m -n 9 -t 4 -p high -- cmd /c \"x y\"");
            }
            other => panic!("{other:?}"),
        }
        assert!(lines.is_empty(), "{lines:?}");
    }

    #[test]
    fn no_job_reports_once_and_stays_plain() {
        let mut lines: Vec<String> = Vec::new();
        let mut front_called = false;
        let mut front = || {
            front_called = true;
            Ok("unused".to_string())
        };
        let mut err = |m: &str| lines.push(m.to_string());
        let out = decide(CallerJob::NoJob, 8, &a(&["cmd"]), &mut front, &mut err);
        assert_eq!(out, Elevation::Plain);
        assert!(!front_called);
        assert_eq!(lines, a(&[NOTHING_TO_KEEP]));
    }

    #[test]
    fn same_front_requires_the_same_file_content() {
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        let exe = here.join("Cargo.toml");
        assert!(same_front(&exe, &exe), "the same path");
        assert!(!same_front(&exe, &here.join("README.md")), "another file");
        assert!(
            !same_front(&exe, &here.join("wnice-no-such-front.exe")),
            "a missing sibling is not our front"
        );
    }

    fn split(v: &[&str]) -> (bool, Vec<String>) {
        let args = a(v);
        let (keep, rest) = split_keep_limits(&args);
        (keep, rest.to_vec())
    }

    #[test]
    fn only_a_leading_keep_limits_is_split_off() {
        assert_eq!(split(&["--keep-limits", "cmd"]), (true, a(&["cmd"])));
        assert_eq!(
            split(&["cmd", "--keep-limits"]),
            (false, a(&["cmd", "--keep-limits"]))
        );
        assert_eq!(split(&["--keep-limits"]), (true, a(&[])));
        assert_eq!(
            split(&["--keep-limits", "--keep-limits", "cmd"]),
            (true, a(&["--keep-limits", "cmd"]))
        );
        assert_eq!(split(&[]), (false, a(&[])));
    }
}
