//! Thin multicall dispatcher: dispatch on `argv[0]` file stem.

use std::process::ExitCode;

fn main() -> ExitCode {
    let arg0 = std::env::args().next().unwrap_or_default();
    let stem = std::path::Path::new(&arg0)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = stem.to_lowercase();

    if stem == "wnice" {
        let argv: Vec<String> = std::env::args().skip(1).collect();
        let sys = std::sync::Arc::new(wnice::RealSys);
        match wnice::cli::parse_args(&argv, wnice::Sys::logical_processor_count(&*sys)) {
            Ok(wnice::cli::Action::Help) => {
                println!("{}", wnice::front::help::text("wnice").unwrap_or_default());
                return ExitCode::from(0);
            }
            Ok(wnice::cli::Action::Version) => {
                println!("wnice {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::from(0);
            }
            Ok(wnice::cli::Action::Run(cfg)) => {
                let code = wnice::run::run(&cfg, sys);
                // process::exit passes the full 32-bit code on Windows, so a
                // negative NTSTATUS from run() passes through bit-cast.
                std::process::exit(code);
            }
            Ok(wnice::cli::Action::Jobs) => {
                std::process::exit(wnice::front::jobs::run());
            }
            Err(wnice::cli::ParseError::Usage(msg)) => {
                eprintln!("wnice: {msg}. {}", wnice::cli::USAGE);
                eprintln!("{}", wnice::front::help::hint("wnice"));
                return ExitCode::from(1);
            }
            Err(
                wnice::cli::ParseError::JobsConflict(msg)
                | wnice::cli::ParseError::NoServersWslConflict(msg)
                | wnice::cli::ParseError::AdoptWslConflict(msg),
            ) => {
                eprintln!("wnice: {msg}");
                return ExitCode::from(2);
            }
        }
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = wnice::front::dispatch(&stem, &args) {
        std::process::exit(code);
    }

    eprintln!("wnice: unknown frontend '{stem}'");
    eprintln!("usage: wnice <options> <command> [args...]");
    ExitCode::from(2)
}
