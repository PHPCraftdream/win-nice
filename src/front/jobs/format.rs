//! Text rendering of a `Scan`: pure functions, no Win32.
//!
//! Column widths are fixed (they never depend on the terminal or the locale),
//! and every line is built by the same cell formatter so headers and rows line
//! up. No line carries trailing spaces. An unknown value renders as `"?"`, so a
//! failed query is never mistaken for "no limit" or "no job".

use super::{
    CapState, CpuCap, JobInfo, LimitFields, Nesting, OwnerFailure, ProcInfo, Scan, Uninspected,
};

const JOB_HEADERS: [&str; 9] = [
    "id", "owner", "command", "cpu cap", "memory", "procs", "affinity", "priority", "active",
];
const JOB_WIDTHS: [usize; 9] = [3, 26, 34, 34, 7, 5, 9, 11, 6];
const PROC_HEADERS: [&str; 5] = ["pid", "image", "cpu all", "cpu one", "jobs"];
const PROC_WIDTHS: [usize; 5] = [6, 24, 7, 7, 44];
/// Content width of the JOBS `command` column.
const COMMAND_WIDTH: usize = JOB_WIDTHS[2];

const GIB: u64 = 1024 * 1024 * 1024;
const MIB: u64 = 1024 * 1024;

/// Footer reasons, in the fixed order they are summarised.
const REASONS: [Uninspected; 6] = [
    Uninspected::DifferentUser,
    Uninspected::AccessDenied,
    Uninspected::TokenQuery,
    Uninspected::HandleTable,
    Uninspected::JobType,
    Uninspected::JobQuery,
];
/// Failures listed one per line before the footer collapses to a count.
const MAX_FAILURES_SHOWN: usize = 20;

/// Render the whole report (one trailing newline).
pub fn render(scan: &Scan) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!(
        "JOBS: {} job object(s) held by wnice-family wrappers",
        scan.jobs.len()
    ));
    lines.push(headers(&JOB_HEADERS, &JOB_WIDTHS));
    for job in &scan.jobs {
        let cells = vec![
            job.id.to_string(),
            format!("{} {}", job.owner_pid, job.owner_image),
            command(job.owner_command.as_deref(), COMMAND_WIDTH),
            cpu_cap(job, scan.logical_cpus),
            memory(job.limits),
            procs(job.limits),
            affinity(job.limits),
            priority(job.limits),
            active(job.active_processes),
        ];
        lines.push(row(&cells, &JOB_WIDTHS));
    }
    if scan
        .jobs
        .iter()
        .any(|job| matches!(job.nesting, Nesting::Nested(_)))
    {
        lines.push(
            "note: nested = share of the parent job's own CPU allowance, not of the whole machine"
                .to_string(),
        );
    }
    lines.push(String::new());
    lines.push(format!(
        "PROCESSES: {} busiest process(es) over {} ms (percent of all logical processors / of one)",
        scan.processes.len(),
        scan.window_ms
    ));
    lines.push(headers(&PROC_HEADERS, &PROC_WIDTHS));
    for proc in &scan.processes {
        let cells = vec![
            proc.pid.to_string(),
            proc.image.clone(),
            format!(
                "{:.1}%",
                percent_all_cores(proc.cpu_seconds, scan.window_ms, scan.logical_cpus)
            ),
            format!("{:.1}%", percent_one_core(proc.cpu_seconds, scan.window_ms)),
            chain(proc, &scan.jobs, scan.logical_cpus, scan.complete),
        ];
        lines.push(row(&cells, &PROC_WIDTHS));
    }
    if !scan.complete {
        lines.push(
            "note: \"inventory incomplete\" = at least one owner process could not be inspected, so a missing or cap-free job chain is not a verdict"
                .to_string(),
        );
    }
    lines.push(String::new());
    lines.extend(footer(&scan.uninspected));
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// The "not inspected" lines: a count plus one reason summary, then the named
/// failures (truncated so a machine full of other-user wrappers stays readable).
fn footer(failures: &[OwnerFailure]) -> Vec<String> {
    if failures.is_empty() {
        return vec!["owner processes not inspected: 0".to_string()];
    }
    let summary: Vec<String> = REASONS
        .iter()
        .filter_map(|reason| {
            let n = failures.iter().filter(|f| f.reason == *reason).count();
            (n > 0).then(|| format!("{n} {}", reason.label()))
        })
        .collect();
    let mut lines = vec![format!(
        "owner processes not inspected: {} ({})",
        failures.len(),
        summary.join(", ")
    )];
    for failure in failures.iter().take(MAX_FAILURES_SHOWN) {
        lines.push(format!(
            "  not inspected: {} {} ({})",
            failure.pid,
            failure.image,
            failure.reason.label()
        ));
    }
    if let Some(more) = failures.len().checked_sub(MAX_FAILURES_SHOWN) {
        if more > 0 {
            lines.push(format!("  ... and {more} more"));
        }
    }
    lines
}

fn headers(names: &[&str], widths: &[usize]) -> String {
    let cells: Vec<String> = names.iter().map(|n| (*n).to_string()).collect();
    row(&cells, widths)
}

/// Left-align `cells` into fixed widths, two spaces between columns.
/// The last cell is never padded, so no line ends in whitespace.
fn row(cells: &[String], widths: &[usize]) -> String {
    let mut line = String::new();
    let last = cells.len().saturating_sub(1);
    for (i, cell) in cells.iter().enumerate() {
        if i > 0 {
            line.push_str("  ");
        }
        line.push_str(cell);
        if i != last {
            let width = widths.get(i).copied().unwrap_or(0);
            let len = cell.chars().count();
            if len < width {
                line.push_str(&" ".repeat(width - len));
            }
        }
    }
    line
}

/// The whole JOBS `cpu cap` cell, including the nesting qualifier.
pub fn cpu_cap(job: &JobInfo, logical_cpus: u32) -> String {
    match (job.cpu, job.nesting) {
        (CapState::Unknown, Nesting::TopLevel) => "?".to_string(),
        (CapState::Unknown, Nesting::Nested(parent)) => format!("nested (job {parent}), cap ?"),
        (CapState::Unknown, Nesting::Unknown) => "? (nesting ?)".to_string(),
        (CapState::Uncapped, Nesting::TopLevel) => "none".to_string(),
        (CapState::Uncapped, Nesting::Nested(parent)) => format!("nested (job {parent}), none"),
        (CapState::Uncapped, Nesting::Unknown) => "none (nesting ?)".to_string(),
        (CapState::Capped(cap), Nesting::TopLevel) => full_cap(cap, logical_cpus),
        // A nested rate is a share of the parent job's allowance, so it says
        // nothing about how much of the machine the tree may use.
        (CapState::Capped(cap), Nesting::Nested(parent)) => {
            format!("nested (job {parent}): {:.1}% of parent", rate(cap))
        }
        (CapState::Capped(cap), Nesting::Unknown) => format!("{} (?)", full_cap(cap, logical_cpus)),
    }
}

/// The same cap, shortened for the PROCESSES `jobs` column.
pub fn cap_short(job: &JobInfo, logical_cpus: u32) -> String {
    match (job.cpu, job.nesting) {
        (CapState::Unknown, _) => "?".to_string(),
        (CapState::Uncapped, _) => "none".to_string(),
        (CapState::Capped(cap), Nesting::TopLevel) => full_cap(cap, logical_cpus),
        (CapState::Capped(cap), Nesting::Nested(parent)) => {
            format!("nested: {:.1}% of job {parent}", rate(cap))
        }
        (CapState::Capped(cap), Nesting::Unknown) => {
            format!("{} (?)", full_cap(cap, logical_cpus))
        }
    }
}

/// `cpu_rate` as a percent (`10000` -> `100.0`).
fn rate(cap: CpuCap) -> f64 {
    cap.cpu_rate as f64 / 100.0
}

/// A top-level cap as `all cores / one core`, in percent.
fn full_cap(cap: CpuCap, logical_cpus: u32) -> String {
    let all = rate(cap);
    let one = all * logical_cpus as f64;
    format!(
        "{all:.1}% all / {one:.1}% one {}",
        if cap.hard { "hard" } else { "soft" }
    )
}

/// Byte count in `g`/`m`/`b`, with the largest exact unit preferred.
pub fn human_bytes(bytes: u64) -> String {
    if bytes.is_multiple_of(GIB) {
        format!("{}g", bytes / GIB)
    } else if bytes >= GIB {
        format!("{:.1}g", bytes as f64 / GIB as f64)
    } else if bytes.is_multiple_of(MIB) {
        format!("{}m", bytes / MIB)
    } else if bytes >= MIB {
        format!("{:.1}m", bytes as f64 / MIB as f64)
    } else {
        format!("{bytes}b")
    }
}

/// The `memory` cell: `"?"` when the limits themselves are unknown.
pub fn memory(limits: Option<LimitFields>) -> String {
    match limits {
        None => "?".to_string(),
        Some(l) => l.job_memory.map_or_else(|| "-".to_string(), human_bytes),
    }
}

/// The `procs` cell (`JOB_OBJECT_LIMIT_ACTIVE_PROCESS`).
pub fn procs(limits: Option<LimitFields>) -> String {
    match limits {
        None => "?".to_string(),
        Some(l) => l
            .active_process_limit
            .map_or_else(|| "-".to_string(), |n| n.to_string()),
    }
}

/// The `affinity` cell, in hex.
pub fn affinity(limits: Option<LimitFields>) -> String {
    match limits {
        None => "?".to_string(),
        Some(l) => l
            .affinity
            .map_or_else(|| "-".to_string(), |mask| format!("0x{mask:x}")),
    }
}

/// The `priority` cell as a wnice name (hex when unknown).
pub fn priority(limits: Option<LimitFields>) -> String {
    match limits {
        None => "?".to_string(),
        Some(l) => priority_name(l.priority),
    }
}

fn priority_name(class: Option<u32>) -> String {
    let name = match class {
        None => "-",
        Some(0x40) => "idle",
        Some(0x4000) => "belownormal",
        Some(0x20) => "normal",
        Some(0x8000) => "abovenormal",
        Some(0x80) => "high",
        Some(0x100) => "realtime",
        Some(other) => return format!("0x{other:x}"),
    };
    name.to_string()
}

/// The `active` cell: `"?"` when the accounting query failed.
pub fn active(count: Option<u32>) -> String {
    count.map_or_else(|| "?".to_string(), |n| n.to_string())
}

/// The `command` cell; `"?"` when the command line could not be read.
pub fn command(text: Option<&str>, width: usize) -> String {
    text.map_or_else(|| "?".to_string(), |c| truncate(c, width))
}

/// Shorten `text` to at most `width` characters using a trailing `"..."`.
pub fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    if width <= 3 {
        return ".".repeat(width);
    }
    let mut out: String = text.chars().take(width - 3).collect();
    out.push_str("...");
    out
}

/// The jobs of `proc`, e.g. `"1 (none) UNCAPPED"`. A verdict is only given
/// for a complete inventory: a failed membership query is never read as "not in
/// a job", and an incomplete one never as "no job at all".
pub fn chain(proc: &ProcInfo, jobs: &[JobInfo], logical_cpus: u32, complete: bool) -> String {
    let Some(ids) = &proc.jobs else {
        return "? (membership unknown)".to_string();
    };
    let found: Vec<&JobInfo> = ids
        .iter()
        .filter_map(|id| jobs.iter().find(|j| j.id == *id))
        .collect();
    if found.is_empty() {
        return if complete {
            "NOT-IN-WNICE-JOB".to_string()
        } else {
            "no wnice job found - inventory incomplete".to_string()
        };
    }
    let entries: Vec<String> = found
        .iter()
        .map(|job| format!("{} ({})", job.id, cap_short(job, logical_cpus)))
        .collect();
    let mut out = entries.join(", ");
    // Only cap-free chains get a verdict, and only when every link is known.
    if !found
        .iter()
        .any(|job| matches!(job.cpu, CapState::Capped(_)))
    {
        if found.iter().all(|job| job.cpu == CapState::Uncapped) {
            out.push_str(if complete {
                " UNCAPPED"
            } else {
                " - inventory incomplete"
            });
        } else {
            out.push_str(" CAP UNKNOWN");
        }
    }
    out
}

/// `cpu_seconds` as a percent of one logical processor over the window.
pub fn percent_one_core(cpu_seconds: f64, window_ms: u64) -> f64 {
    if window_ms == 0 {
        return 0.0;
    }
    cpu_seconds / (window_ms as f64 / 1000.0) * 100.0
}

/// `percent_one_core` divided by the number of logical processors.
pub fn percent_all_cores(cpu_seconds: f64, window_ms: u64, logical_cpus: u32) -> f64 {
    if logical_cpus == 0 {
        return 0.0;
    }
    percent_one_core(cpu_seconds, window_ms) / logical_cpus as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(cpu_rate: u32, hard: bool) -> CapState {
        CapState::Capped(CpuCap { cpu_rate, hard })
    }

    fn job(id: usize, cpu: CapState, nesting: Nesting) -> JobInfo {
        JobInfo {
            id,
            owner_pid: 1000 + id as u32,
            owner_image: "wnice.exe".to_string(),
            owner_command: None,
            cpu,
            limits: None,
            active_processes: None,
            nesting,
        }
    }

    fn proc(id: u32, jobs: Option<Vec<usize>>) -> ProcInfo {
        ProcInfo {
            pid: id,
            image: "node.exe".to_string(),
            cpu_seconds: 0.0,
            jobs,
        }
    }

    fn failure(pid: u32, reason: Uninspected) -> OwnerFailure {
        OwnerFailure {
            pid,
            image: "capc.exe".to_string(),
            reason,
        }
    }

    #[test]
    fn cpu_cap_scales_to_one_core() {
        assert_eq!(
            cpu_cap(&job(1, cap(1000, true), Nesting::TopLevel), 16),
            "10.0% all / 160.0% one hard"
        );
        assert_eq!(
            cpu_cap(&job(1, cap(10000, false), Nesting::TopLevel), 8),
            "100.0% all / 800.0% one soft"
        );
        assert_eq!(
            cpu_cap(&job(1, CapState::Uncapped, Nesting::TopLevel), 8),
            "none"
        );
        // 0 cpus cannot blow up the one-core column.
        assert_eq!(
            cpu_cap(&job(1, cap(5000, true), Nesting::TopLevel), 0),
            "50.0% all / 0.0% one hard"
        );
    }

    #[test]
    fn cpu_cap_covers_every_state_and_nesting() {
        for cpu in [
            CapState::Unknown,
            CapState::Uncapped,
            cap(2500, true),
            cap(2500, false),
        ] {
            let top = cpu_cap(&job(1, cpu, Nesting::TopLevel), 8);
            let nested = cpu_cap(&job(1, cpu, Nesting::Nested(2)), 8);
            let unknown = cpu_cap(&job(1, cpu, Nesting::Unknown), 8);
            // A nested cap is a share of the parent, never a share of the machine.
            assert!(!nested.contains("% all"));
            assert!(!nested.contains("% one"));
            assert!(nested.contains("job 2"));
            assert!(unknown.contains("?"));
            assert!(!top.is_empty());
        }
        assert_eq!(
            cpu_cap(&job(1, CapState::Unknown, Nesting::TopLevel), 8),
            "?"
        );
        assert_eq!(
            cpu_cap(&job(1, CapState::Unknown, Nesting::Nested(3)), 8),
            "nested (job 3), cap ?"
        );
        assert_eq!(
            cpu_cap(&job(1, CapState::Unknown, Nesting::Unknown), 8),
            "? (nesting ?)"
        );
        assert_eq!(
            cpu_cap(&job(1, CapState::Uncapped, Nesting::Nested(3)), 8),
            "nested (job 3), none"
        );
        assert_eq!(
            cpu_cap(&job(1, CapState::Uncapped, Nesting::Unknown), 8),
            "none (nesting ?)"
        );
        assert_eq!(
            cpu_cap(&job(1, cap(5050, false), Nesting::Nested(4)), 8),
            "nested (job 4): 50.5% of parent"
        );
        assert_eq!(
            cpu_cap(&job(1, cap(5050, false), Nesting::Unknown), 8),
            "50.5% all / 404.0% one soft (?)"
        );
    }

    #[test]
    fn cap_short_covers_every_state_and_nesting() {
        assert_eq!(
            cap_short(&job(1, CapState::Unknown, Nesting::TopLevel), 8),
            "?"
        );
        assert_eq!(
            cap_short(&job(1, CapState::Unknown, Nesting::Nested(2)), 8),
            "?"
        );
        assert_eq!(
            cap_short(&job(1, CapState::Uncapped, Nesting::Nested(2)), 8),
            "none"
        );
        assert_eq!(
            cap_short(&job(1, cap(1000, true), Nesting::TopLevel), 16),
            "10.0% all / 160.0% one hard"
        );
        assert_eq!(
            cap_short(&job(1, cap(1000, true), Nesting::Nested(5)), 16),
            "nested: 10.0% of job 5"
        );
        assert_eq!(
            cap_short(&job(1, cap(1000, true), Nesting::Unknown), 16),
            "10.0% all / 160.0% one hard (?)"
        );
        // No short form of a nested cap ever mentions the machine.
        let nested = cap_short(&job(1, cap(1000, true), Nesting::Nested(5)), 16);
        assert!(!nested.contains("% all"));
        assert!(!nested.contains("% one"));
    }

    #[test]
    fn unknown_limits_active_and_command_render_as_question_marks() {
        assert_eq!(memory(None), "?");
        assert_eq!(procs(None), "?");
        assert_eq!(affinity(None), "?");
        assert_eq!(priority(None), "?");
        assert_eq!(active(None), "?");
        assert_eq!(command(None, 34), "?");
        // A known limits block with every member unset keeps the old dashes.
        let empty = Some(LimitFields::default());
        assert_eq!(memory(empty), "-");
        assert_eq!(procs(empty), "-");
        assert_eq!(affinity(empty), "-");
        assert_eq!(priority(empty), "-");
        assert_eq!(active(Some(0)), "0");
        assert_eq!(command(Some("wnice -- x"), 34), "wnice -- x");
    }

    #[test]
    fn human_bytes_picks_the_largest_exact_unit() {
        assert_eq!(human_bytes(2 * GIB), "2g");
        assert_eq!(human_bytes(512 * MIB), "512m");
        assert_eq!(human_bytes(GIB + GIB / 2), "1.5g");
        assert_eq!(human_bytes(4096), "4096b");
        assert_eq!(human_bytes(MIB + MIB / 2), "1.5m");
        assert_eq!(human_bytes(0), "0g");
    }

    #[test]
    fn memory_picks_the_largest_exact_unit() {
        let limits = |bytes| {
            Some(LimitFields {
                job_memory: Some(bytes),
                ..LimitFields::default()
            })
        };
        assert_eq!(memory(limits(2 * GIB)), "2g");
        assert_eq!(memory(limits(512 * MIB)), "512m");
        assert_eq!(memory(limits(MIB + MIB / 2)), "1.5m");
        assert_eq!(memory(limits(4096)), "4096b");
    }

    #[test]
    fn percent_scales() {
        assert_eq!(percent_one_core(1.0, 250), 400.0);
        assert_eq!(percent_all_cores(1.0, 250, 4), 100.0);
        assert_eq!(percent_one_core(1.0, 0), 0.0);
        assert_eq!(percent_all_cores(1.0, 250, 0), 0.0);
        assert_eq!(percent_one_core(0.0, 250), 0.0);
    }

    #[test]
    fn truncate_keeps_whole_chars() {
        assert_eq!(truncate("hello world", 11), "hello world");
        assert_eq!(truncate("hello world", 8), "hello...");
        assert_eq!(truncate("hello", 3), "...");
        assert_eq!(truncate("hello", 1), ".");
        assert_eq!(truncate("hello", 0), "");
        assert_eq!(truncate("日本語テキスト", 6), "日本語...");
        // Never longer than the width, never splitting a char.
        assert_eq!(truncate("日本語テキスト", 5), "日本...");
    }

    #[test]
    fn priority_names() {
        let with = |class| {
            priority(Some(LimitFields {
                priority: Some(class),
                ..LimitFields::default()
            }))
        };
        assert_eq!(priority(Some(LimitFields::default())), "-");
        assert_eq!(with(0x40), "idle");
        assert_eq!(with(0x4000), "belownormal");
        assert_eq!(with(0x20), "normal");
        assert_eq!(with(0x8000), "abovenormal");
        assert_eq!(with(0x80), "high");
        assert_eq!(with(0x100), "realtime");
        assert_eq!(with(0x200), "0x200");
    }

    #[test]
    fn affinity_is_hex() {
        let with = |mask| {
            affinity(Some(LimitFields {
                affinity: Some(mask),
                ..LimitFields::default()
            }))
        };
        assert_eq!(affinity(Some(LimitFields::default())), "-");
        assert_eq!(with(0xffff), "0xffff");
        assert_eq!(with(0), "0x0");
    }

    #[test]
    fn chain_reports_membership_and_caps() {
        let jobs = vec![
            job(1, CapState::Uncapped, Nesting::TopLevel),
            job(2, cap(1000, true), Nesting::TopLevel),
        ];
        // A failed membership query is not "no jobs".
        assert_eq!(
            chain(&proc(1, None), &jobs, 16, true),
            "? (membership unknown)"
        );
        assert_eq!(
            chain(&proc(1, Some(vec![])), &jobs, 16, true),
            "NOT-IN-WNICE-JOB"
        );
        // Unresolvable ids read as "not in a job".
        assert_eq!(
            chain(&proc(1, Some(vec![9])), &jobs, 16, true),
            "NOT-IN-WNICE-JOB"
        );
        assert_eq!(
            chain(&proc(1, Some(vec![1])), &jobs, 16, true),
            "1 (none) UNCAPPED"
        );
        assert_eq!(
            chain(&proc(1, Some(vec![2])), &jobs, 16, true),
            "2 (10.0% all / 160.0% one hard)"
        );
        assert_eq!(
            chain(&proc(1, Some(vec![1, 2])), &jobs, 16, true),
            "1 (none), 2 (10.0% all / 160.0% one hard)"
        );
    }

    #[test]
    fn an_incomplete_inventory_never_gives_a_chain_verdict() {
        let uncapped = vec![job(1, CapState::Uncapped, Nesting::TopLevel)];
        // Empty chain: the verdict needs a complete inventory.
        assert_eq!(
            chain(&proc(1, Some(vec![])), &uncapped, 16, true),
            "NOT-IN-WNICE-JOB"
        );
        let absent = chain(&proc(1, Some(vec![])), &uncapped, 16, false);
        assert_eq!(absent, "no wnice job found - inventory incomplete");
        assert!(!absent.contains("NOT-IN-WNICE-JOB"));
        // Cap-free chain: same rule.
        assert_eq!(
            chain(&proc(1, Some(vec![1])), &uncapped, 16, true),
            "1 (none) UNCAPPED"
        );
        let soft = chain(&proc(1, Some(vec![1])), &uncapped, 16, false);
        assert_eq!(soft, "1 (none) - inventory incomplete");
        assert!(!soft.contains("UNCAPPED"));
        // Unknown membership stays unknown either way.
        assert_eq!(
            chain(&proc(1, None), &uncapped, 16, false),
            "? (membership unknown)"
        );
    }

    #[test]
    fn chain_calls_an_unknown_cap_unknown() {
        let uncapped = job(1, CapState::Uncapped, Nesting::TopLevel);
        let unknown = job(2, CapState::Unknown, Nesting::TopLevel);
        let capped = job(3, cap(1000, true), Nesting::TopLevel);
        let jobs = vec![uncapped, unknown, capped];
        // All uncapped: the chain verdict is a cap verdict.
        assert_eq!(
            chain(&proc(1, Some(vec![1])), &jobs, 16, true),
            "1 (none) UNCAPPED"
        );
        // Mixed uncapped + unknown: no verdict.
        let mixed = chain(&proc(1, Some(vec![1, 2])), &jobs, 16, true);
        assert_eq!(mixed, "1 (none), 2 (?) CAP UNKNOWN");
        assert!(!mixed.contains("UNCAPPED"));
        // One known cap is enough for the verdict to be absent altogether.
        let known = chain(&proc(1, Some(vec![1, 2, 3])), &jobs, 16, true);
        assert!(!known.contains("UNCAPPED"));
        assert!(!known.contains("CAP UNKNOWN"));
        // Nested caps in the chain use the short nested form.
        let nested = vec![job(1, cap(1000, true), Nesting::Nested(7))];
        assert_eq!(
            chain(&proc(1, Some(vec![1])), &nested, 16, true),
            "1 (nested: 10.0% of job 7)"
        );
    }

    fn scan() -> Scan {
        Scan {
            complete: true,
            jobs: vec![
                JobInfo {
                    id: 1,
                    owner_pid: 12345,
                    owner_image: "wnice.exe".to_string(),
                    owner_command: Some("wnice -c 20 -- npm run build".to_string()),
                    cpu: cap(2000, true),
                    limits: Some(LimitFields {
                        job_memory: Some(512 * MIB),
                        active_process_limit: Some(10),
                        affinity: Some(0xffff),
                        priority: Some(0x4000),
                    }),
                    active_processes: Some(3),
                    nesting: Nesting::TopLevel,
                },
                JobInfo {
                    id: 2,
                    owner_pid: 23456,
                    owner_image: "capc.exe".to_string(),
                    owner_command: Some("capc 10 -- node server.js".to_string()),
                    cpu: CapState::Uncapped,
                    limits: None,
                    active_processes: Some(1),
                    nesting: Nesting::TopLevel,
                },
            ],
            processes: vec![
                ProcInfo {
                    pid: 34567,
                    image: "node.exe".to_string(),
                    cpu_seconds: 0.3,
                    jobs: Some(vec![1]),
                },
                ProcInfo {
                    pid: 40000,
                    image: "svchost.exe".to_string(),
                    cpu_seconds: 0.1,
                    jobs: Some(vec![]),
                },
            ],
            uninspected: Vec::new(),
            logical_cpus: 16,
            window_ms: 250,
        }
    }

    #[test]
    fn render_lays_out_both_tables() {
        let want = "\
JOBS: 2 job object(s) held by wnice-family wrappers
id   owner                       command                             cpu cap                             memory   procs  affinity   priority     active
1    12345 wnice.exe             wnice -c 20 -- npm run build        20.0% all / 320.0% one hard         512m     10     0xffff     belownormal  3
2    23456 capc.exe              capc 10 -- node server.js           none                                ?        ?      ?          ?            1

PROCESSES: 2 busiest process(es) over 250 ms (percent of all logical processors / of one)
pid     image                     cpu all  cpu one  jobs
34567   node.exe                  7.5%     120.0%   1 (20.0% all / 320.0% one hard)
40000   svchost.exe               2.5%     40.0%    NOT-IN-WNICE-JOB

owner processes not inspected: 0
";
        assert_eq!(render(&scan()), want);
    }

    #[test]
    fn render_of_an_empty_scan_keeps_the_headers() {
        let text = render(&Scan {
            complete: true,
            ..Scan::default()
        });
        let want = "\
JOBS: 0 job object(s) held by wnice-family wrappers
id   owner                       command                             cpu cap                             memory   procs  affinity   priority     active

PROCESSES: 0 busiest process(es) over 0 ms (percent of all logical processors / of one)
pid     image                     cpu all  cpu one  jobs

owner processes not inspected: 0
";
        assert_eq!(text, want);
    }

    #[test]
    fn render_marks_uninspected_owners() {
        let mut s = scan();
        s.uninspected = vec![
            failure(23456, Uninspected::DifferentUser),
            failure(34567, Uninspected::AccessDenied),
        ];
        s.jobs.clear();
        s.processes.clear();
        let text = render(&s);
        let want = concat!(
            "owner processes not inspected: 2 (1 different user, 1 access denied)\n",
            "  not inspected: 23456 capc.exe (different user)\n",
            "  not inspected: 34567 capc.exe (access denied)\n",
        );
        assert!(text.ends_with(want));
    }

    #[test]
    fn nested_legend_appears_only_for_a_nested_job() {
        let mut s = scan();
        assert!(!render(&s).contains("note: nested"));
        s.jobs[1].nesting = Nesting::Nested(1);
        let text = render(&s);
        assert!(text.contains(
            "\nnote: nested = share of the parent job's own CPU allowance, not of the whole machine\n"
        ));
        assert_eq!(text.matches("note: nested").count(), 1);
        // The legend sits directly under the JOBS rows.
        let line = text
            .lines()
            .position(|l| l.starts_with("note: nested"))
            .unwrap();
        assert!(text
            .lines()
            .nth(line - 1)
            .unwrap()
            .starts_with("2    23456"));
        assert_eq!(text.lines().nth(line + 1), Some(""));
    }

    #[test]
    fn an_incomplete_inventory_adds_one_legend_line() {
        let mut s = scan();
        s.jobs[1].nesting = Nesting::Nested(1);
        s.complete = false;
        let text = render(&s);
        assert_eq!(text.matches("note: \"inventory incomplete\"").count(), 1);
        // The nested legend keeps its own line and is never doubled.
        assert_eq!(text.matches("note: nested").count(), 1);
        // The note follows the PROCESSES rows, just before the footer's blank.
        let lines: Vec<&str> = text.lines().collect();
        let at = lines
            .iter()
            .position(|l| l.starts_with("note: \"inventory incomplete\""))
            .unwrap();
        assert!(lines[at - 1].starts_with("40000"));
        assert_eq!(lines[at + 1], "");
        assert_eq!(lines[at + 2], "owner processes not inspected: 0");
        // A complete scan never shows it.
        s.complete = true;
        assert!(!render(&s).contains("inventory incomplete"));
    }

    #[test]
    fn footer_summarises_reasons_in_the_fixed_order() {
        let scan = Scan {
            complete: true,
            uninspected: vec![
                failure(1, Uninspected::JobQuery),
                failure(2, Uninspected::DifferentUser),
                failure(3, Uninspected::JobQuery),
                failure(4, Uninspected::HandleTable),
            ],
            ..Scan::default()
        };
        assert_eq!(
            footer(&scan.uninspected),
            vec![
                "owner processes not inspected: 4 (1 different user, 1 handle table unavailable, 2 job query failed)",
                "  not inspected: 1 capc.exe (job query failed)",
                "  not inspected: 2 capc.exe (different user)",
                "  not inspected: 3 capc.exe (job query failed)",
                "  not inspected: 4 capc.exe (handle table unavailable)",
            ]
        );
    }

    #[test]
    fn footer_truncates_after_twenty_named_failures() {
        let failures: Vec<OwnerFailure> = (1..=23)
            .map(|pid| failure(pid, Uninspected::TokenQuery))
            .collect();
        let lines = footer(&failures);
        assert_eq!(lines.len(), 1 + MAX_FAILURES_SHOWN + 1);
        assert_eq!(
            lines[0],
            "owner processes not inspected: 23 (23 token query failed)"
        );
        assert_eq!(lines[1], "  not inspected: 1 capc.exe (token query failed)");
        assert_eq!(
            lines[20],
            "  not inspected: 20 capc.exe (token query failed)"
        );
        assert_eq!(lines[21], "  ... and 3 more");
        assert_eq!(
            footer(&[]),
            vec!["owner processes not inspected: 0".to_string()]
        );
    }

    #[test]
    fn an_all_uninspected_scan_names_every_owner() {
        let scan = Scan {
            complete: true,
            jobs: Vec::new(),
            processes: Vec::new(),
            uninspected: vec![
                failure(4100, Uninspected::TokenQuery),
                failure(4200, Uninspected::TokenQuery),
            ],
            logical_cpus: 8,
            window_ms: 250,
        };
        let text = render(&scan);
        assert!(text.contains("JOBS: 0 job object(s)"));
        let want = concat!(
            "owner processes not inspected: 2 (2 token query failed)\n",
            "  not inspected: 4100 capc.exe (token query failed)\n",
            "  not inspected: 4200 capc.exe (token query failed)\n",
        );
        assert!(text.ends_with(want));
    }
}
