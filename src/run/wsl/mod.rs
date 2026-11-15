//! WSL mode: run the wrapped command inside a WSL2 distro under a systemd
//! scope whose cgroup properties are derived from the wnice flags.
//!
//! Split in two halves: the PURE logic lives here (types, flag ->
//! systemd-property mapping, argv assembly, output parsers/decoders, path
//! translation, timeout decision) and never spawns a process, so it is
//! fully unit-testable without WSL; `exec` holds the `WslExec` seam, the
//! real `wsl.exe` driver and the probe flow.
//!
//! `compose` is the single validation choke point (mirroring
//! `limits::compose`): every flag -> property rule and every refusal is
//! decided there, and the flow merely probes facts and calls it.
//!
//! Argument-handling contract: the guest argv is assembled end-to-end as
//! separate vector elements (`wsl.exe --exec`, no shell on either side of
//! the boundary). User arguments are appended verbatim, never quoted,
//! escaped or joined, so shell metacharacters inside them stay inert.

mod exec;
#[cfg(test)]
mod flow_tests;
#[cfg(test)]
mod tests;

pub use exec::run_wsl;

use crate::{Config, MemSpec, PriorityClass};

/// WSL mode selection. `distro: None` means wsl.exe's default distro.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WslMode {
    pub distro: Option<String>,
}

/// Guest facts probed lazily by the flow, only for the flags that need
/// them. An unused field stays 0 and the mapping must treat 0 as "unknown"
/// (the probe was not run) rather than as a real value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuestFacts {
    pub vcpu: u32,
    pub mem_bytes: u64,
}

/// The composed launch: the full `wsl.exe` argv (no shell) plus the systemd
/// unit name (kept for logging/tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WslLaunch {
    pub wsl_args: Vec<String>,
    pub unit: String,
}

/// One row of `wsl -l -v` output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistroEntry {
    pub name: String,
    pub state: String,
    pub version: String,
    pub is_default: bool,
}

/// Refusal for `-p realtime` under WSL. Windows mode silently downgrades
/// realtime to High when the job refuses it; the Linux nice/cgroup-weight
/// model has no realtime band at all, so WSL mode refuses loudly instead of
/// lying about what was applied.
const REALTIME_REFUSAL: &str = "wnice: --wsl: -p realtime is not supported in WSL mode - the Linux nice/cgroup-weight model has no realtime band; drop -p realtime or run without --wsl";

/// (nice value, CPUWeight) for a priority class. Realtime is refused before
/// this is consulted (`guest_command`), so its arm is unreachable.
fn priority_guest_values(p: PriorityClass) -> (&'static str, u32) {
    match p {
        PriorityClass::Idle => ("19", 1),
        PriorityClass::BelowNormal => ("10", 50),
        PriorityClass::Normal => ("0", 100),
        PriorityClass::AboveNormal => ("-5", 500),
        PriorityClass::High => ("-20", 10000),
        PriorityClass::Realtime => unreachable!("realtime refused before mapping"),
    }
}

/// Translate a Windows current directory to its WSL guest path
/// (`C:\a\b` -> `/mnt/c/a/b`, drive lowercased, backslashes -> slashes,
/// `C:\` -> `/mnt/c`). The verbatim prefix is stripped BEFORE
/// classification: `\\?\C:\x` (what `canonicalize` returns, and
/// `current_dir` sometimes) is a plain drive in verbatim form and must
/// translate like `C:\x`. What is refused: verbatim UNC
/// (`\\?\UNC\server\share`), bare UNC (`\\server\share`, `//server/share`)
/// and anything without drive-letter + separator semantics — wnice never
/// guesses a mapping it cannot verify, and `test -d` in the guest would
/// only catch a wrong guess later with a confusing message.
pub fn translate_windows_cwd(cwd: &str) -> Result<String, String> {
    let unc = || {
        format!("wnice: --wsl: cannot translate the current directory (UNC paths are not supported in WSL mode): {cwd}")
    };
    let not_drive =
        || format!("wnice: --wsl: current directory is not a Windows drive path: {cwd}");
    if cwd.starts_with(r"\\?\UNC\") {
        return Err(unc());
    }
    let body = cwd.strip_prefix(r"\\?\").unwrap_or(cwd);
    // Still separator-initial after the optional verbatim strip: a bare
    // UNC path (or a non-drive verbatim form) — no guest mapping, refuse.
    if body.starts_with('\\') || body.starts_with('/') {
        return Err(unc());
    }
    let mut chars = body.chars();
    let Some(drive) = chars.next() else {
        return Err(not_drive());
    };
    if !drive.is_ascii_alphabetic() || chars.next() != Some(':') {
        return Err(not_drive());
    }
    // A drive letter must be followed by a separator ("C:" bare has no
    // guest meaning; both separators are accepted because the Win32
    // current-dir APIs return backslashes but users may pass either).
    match chars.next() {
        Some('\\') | Some('/') => {}
        _ => return Err(not_drive()),
    }
    let drive = drive.to_ascii_lowercase();
    let tail = body[3..].replace('\\', "/");
    if tail.is_empty() {
        Ok(format!("/mnt/{drive}"))
    } else {
        Ok(format!("/mnt/{drive}/{tail}"))
    }
}

/// systemd scope unit name for this run. Both inputs are integers produced
/// by the flow (`std::process::id()`, UNIX epoch millis), so the result
/// only ever contains digits and dashes — valid in a systemd unit name
/// without further escaping.
pub fn unit_name(win_pid: u32, unix_millis: u128) -> String {
    format!("wnice-{win_pid}-{unix_millis}")
}

/// Decode `wsl.exe` output bytes. Real wsl.exe prints UTF-16LE (usually
/// with a BOM); probes that run Linux commands print UTF-8. Order of
/// detection: explicit BOM first, then a NUL-density heuristic (a >25% NUL
/// share in an even-length buffer is UTF-16LE — UTF-8 text is virtually
/// NUL-free), then plain UTF-8 (lossy so probe output can never crash the
/// flow).
pub fn decode_wsl_output(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return utf16le_lossy(&bytes[2..]);
    }
    let nuls = bytes.iter().filter(|&&b| b == 0).count();
    if !bytes.is_empty() && bytes.len().is_multiple_of(2) && nuls * 4 > bytes.len() {
        return utf16le_lossy(bytes);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// Decode UTF-16LE bytes (an odd trailing byte is dropped), replacing
/// invalid units per the lossy contract. Hand-rolled pair iteration:
/// `chunks_exact(2)` trips clippy's chunks_exact_to_as_chunks lint on new
/// stable, and its suggested fix `as_chunks` needs Rust 1.88 while this
/// crate's MSRV is 1.87.
fn utf16le_lossy(bytes: &[u8]) -> String {
    let mut units = Vec::with_capacity(bytes.len() / 2);
    let mut i = 0;
    while i + 1 < bytes.len() {
        units.push(u16::from_le_bytes([bytes[i], bytes[i + 1]]));
        i += 2;
    }
    String::from_utf16_lossy(&units)
}

/// Parse `wsl -l -v` output (already decoded). Empty lines and the header
/// line are skipped; garbage lines with fewer than 2 tokens are skipped.
/// The name keeps internal spaces ("Ubuntu 22.04 LTS") by taking everything
/// before the state token rather than a fixed column; a leading `*` marks
/// the default distro.
pub fn parse_distro_list(text: &str) -> Vec<DistroEntry> {
    let mut out = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("NAME") || (trimmed.contains("STATE") && trimmed.contains("VERSION"))
        {
            continue;
        }
        let line_end = line.trim_end();
        let tokens: Vec<&str> = line_end.split_whitespace().collect();
        if tokens.len() < 2 {
            continue;
        }
        let version = tokens[tokens.len() - 1];
        let state = tokens[tokens.len() - 2];
        // Everything before the state token, from the trim_end'ed line:
        // strip the trailing state token off, leaving only the name (plus
        // the optional default marker).
        let before_version = &line_end[..line_end.len() - version.len()];
        let before_state = before_version.trim_end();
        let mut name = before_state[..before_state.len() - state.len()].trim();
        let is_default = name.starts_with('*');
        if is_default {
            name = name[1..].trim_start();
        }
        out.push(DistroEntry {
            name: name.to_string(),
            state: state.to_string(),
            version: version.to_string(),
            is_default,
        });
    }
    out
}

/// Pick the distro to use: the starred default when `want` is None, else an
/// exact (then ASCII-case-insensitive) name match. The WSL2 version check
/// is deliberately NOT here — the flow owns it so it can name the found
/// distro in the message.
pub fn pick_distro<'a>(
    list: &'a [DistroEntry],
    want: Option<&str>,
) -> Result<&'a DistroEntry, String> {
    match want {
        None => list.iter().find(|d| d.is_default).ok_or_else(|| {
            "wnice: --wsl: no default WSL distribution found (installed: none)".to_string()
        }),
        Some(want) => list
            .iter()
            .find(|d| d.name == want)
            .or_else(|| list.iter().find(|d| d.name.eq_ignore_ascii_case(want)))
            .ok_or_else(|| {
                format!(
                    "wnice: --wsl: WSL distribution '{want}' not found (available: {})",
                    list.iter()
                        .map(|d| d.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }),
    }
}

/// First `MemTotal: <n> kB` line of /proc/meminfo text, in kB.
pub fn parse_meminfo_kb(text: &str) -> Option<u64> {
    for line in text.lines() {
        let rest = match line.trim_start().strip_prefix("MemTotal:") {
            Some(r) => r,
            None => continue,
        };
        let mut tokens = rest.split_whitespace();
        let (Some(kb_tok), Some(unit)) = (tokens.next(), tokens.next()) else {
            continue;
        };
        if unit != "kB" {
            continue;
        }
        if let Ok(kb) = kb_tok.parse::<u64>() {
            return Some(kb);
        }
    }
    None
}

/// `nproc --all` output: a trimmed u32 greater than 0.
pub fn parse_nproc(text: &str) -> Option<u32> {
    text.trim().parse::<u32>().ok().filter(|&n| n > 0)
}

/// Human-facing timeout length for the timeout message — same algorithm as
/// `run::timeout_display` (integer seconds when exact, otherwise seconds
/// with up to 3 decimals, trailing zeros trimmed). Implemented locally on
/// purpose: `run/mod.rs`'s copy belongs to the Windows engine and must not
/// be touched for WSL mode.
pub fn format_seconds_display(ms: u32) -> String {
    if ms.is_multiple_of(1000) {
        return format!("{}s", ms / 1000);
    }
    let s = format!("{:.3}", ms as f64 / 1000.0);
    format!("{}s", s.trim_end_matches('0').trim_end_matches('.'))
}

/// The Windows-side timeout decision for the WSL final run: `Some(124)` only
/// when the deadline has been reached (the scope was already killed inside
/// the guest by `RuntimeMaxSec` + SIGKILL; 124 is the unix timeout(1)
/// convention, as in Windows mode). `None` keeps the child's exit code.
/// Split out so tests can drive it without a real child.
pub fn timeout_exit(_code: i32, elapsed_ms: u64, timeout_ms: u32) -> Option<i32> {
    if elapsed_ms >= u64::from(timeout_ms) {
        Some(124)
    } else {
        None
    }
}

/// The guest-side command tail: `nice -n <nice>` (only with a priority)
/// wrapping `runuser -u <user> -- <cmd...>`, every element separate — this
/// is what makes shell metacharacters in user arguments inert. Errors only
/// for the realtime refusal (shared with `compose`, which calls this).
pub fn guest_command(cfg: &Config, user: &str) -> Result<Vec<String>, String> {
    if cfg.priority == Some(PriorityClass::Realtime) {
        return Err(REALTIME_REFUSAL.to_string());
    }
    let mut tail: Vec<String> = Vec::new();
    if let Some(p) = cfg.priority {
        let (nice, _) = priority_guest_values(p);
        tail.extend(["nice".to_string(), "-n".to_string(), nice.to_string()]);
    }
    tail.extend([
        "runuser".to_string(),
        "-u".to_string(),
        user.to_string(),
        "--".to_string(),
    ]);
    tail.extend(cfg.command.iter().cloned());
    Ok(tail)
}

/// Resolve a [`MemSpec`] to an absolute byte count for `MemoryMax`. Percent
/// is the floor of `mem_bytes * percent / 100`; a 0 result (unknown guest
/// memory or a fraction too small to round up) is refused rather than sent
/// as `MemoryMax=0`, which systemd would read as "no limit".
fn resolve_guest_memory(mem: MemSpec, facts: &GuestFacts) -> Result<u64, String> {
    match mem {
        MemSpec::Bytes(b) => Ok(b),
        MemSpec::Percent(p) => {
            let bytes = (u128::from(facts.mem_bytes) * u128::from(p) / 100) as u64;
            if bytes == 0 {
                return Err(
                    "wnice: --wsl: memory limit is too small - rounds to 0 bytes".to_string(),
                );
            }
            Ok(bytes)
        }
    }
}

/// Compose a [`Config`] into a [`WslLaunch`]: the `wsl.exe` argv for a
/// systemd scope with one `-p` property per requested flag, then `--`, the
/// nice wrapper (when a priority is set), runuser and the user command.
/// The single validation choke point: realtime is refused via
/// [`guest_command`], `-c`/`-t` need a probed vCPU count, and percent
/// memory needs probed guest memory (see [`GuestFacts`]).
pub fn compose(
    cfg: &Config,
    facts: &GuestFacts,
    user: &str,
    cwd_guest: &str,
    unit: &str,
) -> Result<WslLaunch, String> {
    // The command tail is validated first so the realtime refusal wins over
    // any later property error deterministically.
    let tail = guest_command(cfg, user)?;

    let mut args: Vec<String> = Vec::new();
    if let Some(d) = cfg.wsl.as_ref().and_then(|m| m.distro.as_ref()) {
        args.extend(["-d".to_string(), d.clone()]);
    }
    args.extend(["-u".to_string(), "root".to_string()]);
    args.extend(["--cd".to_string(), cwd_guest.to_string()]);
    args.extend([
        "--exec".to_string(),
        "systemd-run".to_string(),
        "--scope".to_string(),
        "--quiet".to_string(),
        "--collect".to_string(),
        format!("--unit={unit}"),
    ]);

    if let Some(n) = cfg.cpu_percent {
        if facts.vcpu == 0 {
            return Err(
                "wnice: --wsl: -c/--cpu requires the guest vCPU count (nproc was not probed)"
                    .to_string(),
            );
        }
        args.push("-p".to_string());
        // N percent of ALL guest vCPUs, as CPUQuota sees it.
        args.push(format!(
            "CPUQuota={}%",
            u64::from(n) * u64::from(facts.vcpu)
        ));
    }
    if let Some(p) = cfg.priority {
        let (_, weight) = priority_guest_values(p);
        args.push("-p".to_string());
        args.push(format!("CPUWeight={weight}"));
    }
    if let Some(m) = cfg.memory {
        let bytes = resolve_guest_memory(m, facts)?;
        args.push("-p".to_string());
        args.push(format!("MemoryMax={bytes}"));
    }
    if let Some(n) = cfg.threads {
        if facts.vcpu == 0 || n == 0 || n > facts.vcpu {
            return Err(format!(
                "wnice: --wsl: -t/--threads {n} exceeds the guest's {} vCPUs",
                facts.vcpu
            ));
        }
        args.push("-p".to_string());
        let cpus = if n == 1 {
            "0".to_string()
        } else {
            format!("0-{}", n - 1)
        };
        args.push(format!("AllowedCPUs={cpus}"));
    }
    if let Some(n) = cfg.procs {
        args.push("-p".to_string());
        args.push(format!("TasksMax={n}"));
    }
    if let Some(ms) = cfg.timeout_ms {
        args.push("-p".to_string());
        args.push(format!("RuntimeMaxSec={ms}ms"));
        // Cgroup-wide SIGKILL at the deadline: the closest analog of
        // TerminateJobObject, since the scope IS the job inside the guest.
        args.push("-p".to_string());
        args.push("KillSignal=SIGKILL".to_string());
    }

    args.push("--".to_string());
    args.extend(tail);
    Ok(WslLaunch {
        wsl_args: args,
        unit: unit.to_string(),
    })
}
