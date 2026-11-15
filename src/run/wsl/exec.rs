//! The execution half of WSL mode: the `WslExec` seam, the real `wsl.exe`
//! driver, the precheck probes and the run flow.
//!
//! Everything goes through `wsl.exe` with argv passed as separate elements
//! (no shell on either side). Precheck probes run with piped output and a
//! 120 s deadline so a wedged VM cannot hang wnice forever; the final
//! command inherits the console and runs WITHOUT a timeout — `-s` is
//! enforced inside the guest by the systemd scope, and the 124 decision is
//! made Windows-side afterwards via [`timeout_exit`].

use std::time::{Duration, Instant};

use super::GuestFacts;
use super::{
    compose, decode_wsl_output, format_seconds_display, parse_distro_list, parse_meminfo_kb,
    parse_nproc, pick_distro, timeout_exit, translate_windows_cwd, unit_name,
};
use crate::{Config, MemSpec, Sys};

/// How often the capture path polls `try_wait` while reader threads drain
/// the pipes.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Deadline for every precheck probe: wsl.exe's first call may start the VM
/// and is slow, but must never hang forever.
const PROBE_TIMEOUT: Duration = Duration::from_secs(120);

/// Outcome of one `wsl.exe` invocation. Raw bytes on purpose — the decoder
/// ([`decode_wsl_output`]) decides the encoding, since wsl.exe prints
/// UTF-16LE while Linux commands print UTF-8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecResult {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Stdio strategy for one `wsl.exe` invocation. `Capture` pipes
/// stdout/stderr (stdin null) for probes; `Inherit` hands all three stdio
/// streams to the wrapped command, which is the whole point of the final
/// run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecMode {
    Capture,
    Inherit,
}

/// Seam over "run wsl.exe" so the flow can be tested with a scripted fake
/// (no WSL required).
pub trait WslExec {
    fn run(
        &self,
        wsl_exe: &str,
        args: &[String],
        mode: ExecMode,
        timeout: Option<Duration>,
    ) -> Result<ExecResult, String>;
}

/// Real driver: spawns `wsl.exe` directly (NO shell). The crate is
/// Windows-only, so `CommandExt::creation_flags` is used unconditionally to
/// set CREATE_NO_WINDOW — the project convention that nothing opens
/// windows.
pub struct RealExec;

impl WslExec for RealExec {
    fn run(
        &self,
        wsl_exe: &str,
        args: &[String],
        mode: ExecMode,
        timeout: Option<Duration>,
    ) -> Result<ExecResult, String> {
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};

        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let mut cmd = Command::new(wsl_exe);
        cmd.args(args).creation_flags(CREATE_NO_WINDOW);
        match mode {
            ExecMode::Inherit => {
                cmd.stdin(Stdio::inherit())
                    .stdout(Stdio::inherit())
                    .stderr(Stdio::inherit());
            }
            ExecMode::Capture => {
                cmd.stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
            }
        }
        let mut child = cmd.spawn().map_err(|e| e.to_string())?;

        if mode == ExecMode::Inherit {
            // No timeout, no pipes: the wrapped command owns the console
            // and may legitimately run for hours.
            let status = child.wait().map_err(|e| e.to_string())?;
            return Ok(ExecResult {
                code: status.code().unwrap_or(-1),
                stdout: Vec::new(),
                stderr: Vec::new(),
            });
        }

        // Capture path: reader threads drain the pipes so a chatty probe
        // can never fill the OS pipe buffer and deadlock against the
        // polling loop below.
        let deadline = timeout.map(|t| Instant::now() + t);
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();
        let out_reader = std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = stdout_pipe {
                let _ = std::io::Read::read_to_end(&mut p, &mut buf);
            }
            buf
        });
        let err_reader = std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = stderr_pipe {
                let _ = std::io::Read::read_to_end(&mut p, &mut buf);
            }
            buf
        });

        let status = loop {
            match child.try_wait() {
                Ok(Some(st)) => break st,
                Ok(None) => {
                    if deadline.is_some_and(|d| Instant::now() >= d) {
                        // Probe deadline: kill, reap, drain, report.
                        let _ = child.kill();
                        let _ = child.wait();
                        let _ = out_reader.join();
                        let _ = err_reader.join();
                        let secs = timeout.map(|t| t.as_secs()).unwrap_or(0);
                        return Err(format!(
                            "wnice: --wsl: WSL probe timed out after {secs}s - killed"
                        ));
                    }
                    std::thread::sleep(POLL_INTERVAL);
                }
                Err(e) => return Err(e.to_string()),
            }
        };

        let stdout = out_reader.join().unwrap_or_default();
        let stderr = err_reader.join().unwrap_or_default();
        Ok(ExecResult {
            code: status.code().unwrap_or(-1),
            stdout,
            stderr,
        })
    }
}

/// `<system_dir>\wsl.exe` (backslash join; an empty system dir degrades to
/// a bare `wsl.exe` so PATH lookup still applies).
pub fn system32_wsl_path(system_dir: &str) -> String {
    if system_dir.is_empty() {
        return "wsl.exe".to_string();
    }
    format!("{system_dir}\\wsl.exe")
}

/// Small helper keeping the probe call sites readable.
fn argv(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// One precheck probe: piped output, [`PROBE_TIMEOUT`] deadline.
fn probe(exec: &dyn WslExec, wsl_exe: &str, args: &[String]) -> Result<ExecResult, String> {
    exec.run(wsl_exe, args, ExecMode::Capture, Some(PROBE_TIMEOUT))
}

/// First ~200 characters of a probe's decoded output (stdout preferred,
/// stderr fallback), for inclusion in error messages.
fn output_snippet(res: &ExecResult) -> String {
    let mut text = decode_wsl_output(&res.stdout).trim().to_string();
    if text.is_empty() {
        text = decode_wsl_output(&res.stderr).trim().to_string();
    }
    text.chars().take(200).collect()
}

/// Entry point used by `run::run_with_writer_in`: resolves wsl.exe from the
/// real system directory and the real current directory, then delegates to
/// [`run_wsl_in`].
pub fn run_wsl(cfg: &Config, err: &mut dyn FnMut(&str)) -> i32 {
    let wsl_exe = system32_wsl_path(&crate::RealSys.system_directory());
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    run_wsl_in(cfg, &RealExec, &wsl_exe, &cwd, err)
}

/// The WSL flow with everything injected (exec seam, wsl.exe path, Windows
/// cwd, stderr writer), so tests script the probes without WSL.
pub fn run_wsl_in(
    cfg: &Config,
    exec: &dyn WslExec,
    wsl_exe: &str,
    cwd_win: &str,
    err: &mut dyn FnMut(&str),
) -> i32 {
    if cfg.command.is_empty() {
        err("wnice: --wsl: no command given");
        return 1;
    }
    let guest_cwd = match translate_windows_cwd(cwd_win) {
        Ok(c) => c,
        Err(msg) => {
            err(&msg);
            return 1;
        }
    };

    // Which distro?
    let list = probe(exec, wsl_exe, &argv(&["--list", "--verbose"]));
    let list = match list {
        Ok(r) if r.code == 0 => r,
        Ok(r) => {
            err(&format!(
                "wnice: --wsl: failed to query WSL distributions: {}",
                output_snippet(&r)
            ));
            return 1;
        }
        Err(detail) => {
            err(&format!(
                "wnice: --wsl: failed to query WSL distributions: {detail}"
            ));
            return 1;
        }
    };
    let distros = parse_distro_list(&decode_wsl_output(&list.stdout));
    let wanted = cfg.wsl.as_ref().and_then(|m| m.distro.as_deref());
    let entry = match pick_distro(&distros, wanted) {
        Ok(e) => e,
        Err(msg) => {
            err(&msg);
            return 1;
        }
    };
    let name = entry.name.clone();
    if entry.version != "2" {
        err(&format!(
            "wnice: --wsl: WSL distribution '{name}' is WSL{}, not WSL2",
            entry.version
        ));
        return 1;
    }

    // systemd as PID 1 (systemd-run cannot work without it).
    let pid1 = probe(
        exec,
        wsl_exe,
        &argv(&["-d", &name, "-u", "root", "--exec", "cat", "/proc/1/comm"]),
    );
    let comm = match &pid1 {
        Ok(r) => decode_wsl_output(&r.stdout).trim().to_string(),
        Err(_) => "(unknown)".to_string(),
    };
    let pid1_ok =
        matches!(&pid1, Ok(r) if r.code == 0 && decode_wsl_output(&r.stdout).trim() == "systemd");
    if !pid1_ok {
        err(&format!("wnice: --wsl: WSL distribution '{name}' does not run systemd as PID 1 (got '{comm}'); set [boot] systemd=true in /etc/wsl.conf and restart WSL"));
        return 1;
    }

    // cgroup v2 (the CPUWeight/TasksMax/MemoryMax properties need it).
    let cgroup = probe(
        exec,
        wsl_exe,
        &argv(&[
            "-d",
            &name,
            "-u",
            "root",
            "--exec",
            "cat",
            "/sys/fs/cgroup/cgroup.controllers",
        ]),
    );
    let cgroup_ok =
        matches!(&cgroup, Ok(r) if r.code == 0 && !decode_wsl_output(&r.stdout).trim().is_empty());
    if !cgroup_ok {
        err(&format!(
            "wnice: --wsl: cgroup v2 is not available in '{name}'"
        ));
        return 1;
    }

    // systemd-run binary present.
    let sdrun = probe(
        exec,
        wsl_exe,
        &argv(&[
            "-d",
            &name,
            "-u",
            "root",
            "--exec",
            "systemd-run",
            "--version",
        ]),
    );
    let sdrun_ok = matches!(&sdrun, Ok(r) if r.code == 0);
    if !sdrun_ok {
        err(&format!(
            "wnice: --wsl: systemd-run is not available in '{name}' (is systemd fully installed?)"
        ));
        return 1;
    }

    // The distro's default user (NO -u flag: runs as whoever the distro
    // config names), which runuser will drop to.
    let user_res = probe(exec, wsl_exe, &argv(&["-d", &name, "--exec", "id", "-un"]));
    let user = match &user_res {
        Ok(r) if r.code == 0 => {
            let u = decode_wsl_output(&r.stdout).trim().to_string();
            if u.is_empty() {
                None
            } else {
                Some(u)
            }
        }
        _ => None,
    };
    let Some(user) = user else {
        let detail = match &user_res {
            Ok(r) => output_snippet(r),
            Err(detail) => detail.clone(),
        };
        err(&format!(
            "wnice: --wsl: failed to determine the default guest user of '{name}': {detail}"
        ));
        return 1;
    };

    // The exact nice/runuser chain the launch will use, dry-run as root.
    let chain = probe(
        exec,
        wsl_exe,
        &argv(&[
            "-d",
            &name,
            "-u",
            "root",
            "--exec",
            "nice",
            "-n",
            "1",
            "runuser",
            "-u",
            &user,
            "--",
            "/bin/true",
        ]),
    );
    let chain_code = match &chain {
        Ok(r) => r.code,
        Err(_) => -1,
    };
    if chain_code != 0 {
        let stderr_suffix = match &chain {
            Ok(r) => {
                let s = decode_wsl_output(&r.stderr).trim().to_string();
                if s.is_empty() {
                    String::new()
                } else {
                    format!(": {}", s.chars().take(200).collect::<String>())
                }
            }
            Err(_) => String::new(),
        };
        err(&format!(
            "wnice: --wsl: 'nice runuser -u {user}' failed inside '{name}' (exit {chain_code}){stderr_suffix}"
        ));
        return 1;
    }

    // The translated cwd must already exist inside the guest (drvfs mount).
    let cwd_probe = probe(
        exec,
        wsl_exe,
        &argv(&[
            "-d", &name, "-u", "root", "--exec", "test", "-d", &guest_cwd,
        ]),
    );
    let cwd_ok = matches!(&cwd_probe, Ok(r) if r.code == 0);
    if !cwd_ok {
        err(&format!("wnice: --wsl: the current Windows directory is not visible in the guest ({guest_cwd} missing - is the drive mounted?)"));
        return 1;
    }

    // Lazy guest facts, only for the flags that consume them.
    let mut facts = GuestFacts {
        vcpu: 0,
        mem_bytes: 0,
    };
    if cfg.cpu_percent.is_some() || cfg.threads.is_some() {
        let nproc = probe(
            exec,
            wsl_exe,
            &argv(&["-d", &name, "-u", "root", "--exec", "nproc", "--all"]),
        );
        facts.vcpu = match &nproc {
            Ok(r) => parse_nproc(&decode_wsl_output(&r.stdout)).unwrap_or(0),
            Err(_) => 0,
        };
        if facts.vcpu == 0 {
            err("wnice: --wsl: failed to determine the guest vCPU count");
            return 1;
        }
    }
    if matches!(cfg.memory, Some(MemSpec::Percent(_))) {
        let meminfo = probe(
            exec,
            wsl_exe,
            &argv(&["-d", &name, "-u", "root", "--exec", "cat", "/proc/meminfo"]),
        );
        facts.mem_bytes = match &meminfo {
            Ok(r) => parse_meminfo_kb(&decode_wsl_output(&r.stdout))
                .unwrap_or(0)
                .saturating_mul(1024),
            Err(_) => 0,
        };
        if facts.mem_bytes == 0 {
            err("wnice: --wsl: failed to determine the guest memory size");
            return 1;
        }
    }

    // Compose the systemd-run argv (validation choke point) and run it.
    let pid = std::process::id();
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let unit = unit_name(pid, millis);
    let launch = match compose(cfg, &facts, &user, &guest_cwd, &unit) {
        Ok(l) => l,
        Err(msg) => {
            err(&msg);
            return 1;
        }
    };

    // Elapsed is measured around the final run only: the probes' cost is
    // not part of the wrapped command's runtime.
    let started = Instant::now();
    let res = match exec.run(wsl_exe, &launch.wsl_args, ExecMode::Inherit, None) {
        Ok(r) => r,
        Err(detail) => {
            err(&format!("wnice: --wsl: failed to run wsl.exe: {detail}"));
            return 1;
        }
    };

    if let Some(tmo) = cfg.timeout_ms {
        let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        if timeout_exit(res.code, elapsed_ms, tmo).is_some() {
            err(&format!(
                "wnice: timed out after {} - the systemd scope was killed inside WSL",
                format_seconds_display(tmo)
            ));
            return 124;
        }
    }
    res.code
}
