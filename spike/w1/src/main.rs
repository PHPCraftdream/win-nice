//! w1-spike: Win32 Job Object + priority-boost spike crate (thin dispatcher).

mod common;
mod exp1;
mod exp2;
mod exp34;

use common::kv;

fn cmd_env() -> i32 {
    // Registry-free, GetVersionExW-free: OS family from std, pid from Win32.
    // OS build number intentionally skipped.
    kv("os", std::env::consts::OS);
    kv("pid", unsafe { windows_sys::Win32::System::Threading::GetCurrentProcessId() });
    kv("is_elevated", common::is_elevated());
    kv("logical_cpus", common::logical_cpus());
    kv("self_exe", common::self_exe());
    0
}

fn usage() -> i32 {
    kv("usage", "w1-spike <env|selfset|exp1|exp2-latency|exp2-chain|chain|wslprobe|wnrun|burner|lat-probe|leaf|leafprio> [args]");
    2
}

fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(|s| s.as_str()) {
        None => usage(),
        Some("env") => cmd_env(),
        Some("selfset") | Some("exp2-latency") | Some("exp2-chain") | Some("chain")
        | Some("lat-probe") | Some("leaf") => exp2::dispatch(&args),
        Some("exp1") | Some("burner") => exp1::dispatch(&args),
        Some("wslprobe") | Some("wnrun") | Some("leafprio") => exp34::dispatch(&args),
        Some(_) => {
            kv("error", format!("unknown subcommand '{}'", args[0]));
            usage()
        }
    }
}

fn main() {
    std::process::exit(run());
}
