//! Text rendering of a [`Scan`]: pure functions, no Win32.
//!
//! Column widths are fixed (they never depend on the terminal or the locale),
//! and every line is built by the same cell formatter so headers and rows line
//! up. No line carries trailing spaces.

use super::{CpuCap, JobInfo, ProcInfo, Scan};

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
            job.owner_command
                .as_deref()
                .map_or_else(|| "-".to_string(), |c| truncate(c, COMMAND_WIDTH)),
            cpu_cap(job.cpu, scan.logical_cpus),
            memory(job.job_memory),
            job.active_process_limit
                .map_or_else(|| "-".to_string(), |n| n.to_string()),
            affinity(job.affinity),
            priority(job.priority),
            job.active_processes.to_string(),
        ];
        lines.push(row(&cells, &JOB_WIDTHS));
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
            chain(proc, &scan.jobs, scan.logical_cpus),
        ];
        lines.push(row(&cells, &PROC_WIDTHS));
    }
    lines.push(String::new());
    lines.push(footer(scan.uninspected_owners));
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// The "not inspected" footer; the reason hint only appears when it matters.
fn footer(n: usize) -> String {
    if n == 0 {
        format!("owner processes not inspected: {n}")
    } else {
        format!("owner processes not inspected: {n} (protected, elevated or another user)")
    }
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

/// CPU cap as `all cores / one core`, in percent.
pub fn cpu_cap(cap: Option<CpuCap>, logical_cpus: u32) -> String {
    match cap {
        None => "none".to_string(),
        Some(c) => {
            let all = c.cpu_rate as f64 / 100.0;
            let one = all * logical_cpus as f64;
            format!(
                "{all:.1}% all / {one:.1}% one {}",
                if c.hard { "hard" } else { "soft" }
            )
        }
    }
}

/// Byte count in `g`/`m`/`b`, with the largest exact unit preferred.
pub fn memory(bytes: Option<u64>) -> String {
    match bytes {
        None => "-".to_string(),
        Some(b) => {
            if b.is_multiple_of(GIB) {
                format!("{}g", b / GIB)
            } else if b >= GIB {
                format!("{:.1}g", b as f64 / GIB as f64)
            } else if b.is_multiple_of(MIB) {
                format!("{}m", b / MIB)
            } else if b >= MIB {
                format!("{:.1}m", b as f64 / MIB as f64)
            } else {
                format!("{b}b")
            }
        }
    }
}

/// Processor affinity mask in hex.
pub fn affinity(mask: Option<u64>) -> String {
    match mask {
        None => "-".to_string(),
        Some(m) => format!("0x{m:x}"),
    }
}

/// A `PRIORITY_CLASS_*` value as its wnice name (hex when unknown).
pub fn priority(class: Option<u32>) -> String {
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

/// The jobs of `proc`, e.g. `"1 (none) UNCAPPED"`.
pub fn chain(proc: &ProcInfo, jobs: &[JobInfo], logical_cpus: u32) -> String {
    let found: Vec<&JobInfo> = proc
        .jobs
        .iter()
        .filter_map(|id| jobs.iter().find(|j| j.id == *id))
        .collect();
    if found.is_empty() {
        return "NOT-IN-WNICE-JOB".to_string();
    }
    let entries: Vec<String> = found
        .iter()
        .map(|job| format!("{} ({})", job.id, cpu_cap(job.cpu, logical_cpus)))
        .collect();
    let mut out = entries.join(", ");
    if !found.iter().any(|job| job.cpu.is_some()) {
        out.push_str(" UNCAPPED");
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

/// [`percent_one_core`] divided by the number of logical processors.
pub fn percent_all_cores(cpu_seconds: f64, window_ms: u64, logical_cpus: u32) -> f64 {
    if logical_cpus == 0 {
        return 0.0;
    }
    percent_one_core(cpu_seconds, window_ms) / logical_cpus as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(cpu_rate: u32, hard: bool) -> Option<CpuCap> {
        Some(CpuCap { cpu_rate, hard })
    }

    fn job(id: usize, cpu: Option<CpuCap>) -> JobInfo {
        JobInfo {
            id,
            owner_pid: 1000 + id as u32,
            owner_image: "wnice.exe".to_string(),
            owner_command: None,
            cpu,
            job_memory: None,
            active_process_limit: None,
            affinity: None,
            priority: None,
            active_processes: 0,
        }
    }

    fn proc(id: u32, jobs: Vec<usize>) -> ProcInfo {
        ProcInfo {
            pid: id,
            image: "node.exe".to_string(),
            cpu_seconds: 0.0,
            jobs,
        }
    }

    #[test]
    fn cpu_cap_scales_to_one_core() {
        assert_eq!(cpu_cap(cap(1000, true), 16), "10.0% all / 160.0% one hard");
        assert_eq!(
            cpu_cap(cap(10000, false), 8),
            "100.0% all / 800.0% one soft"
        );
        assert_eq!(cpu_cap(None, 8), "none");
        // 0 cpus cannot blow up the one-core column.
        assert_eq!(cpu_cap(cap(5000, true), 0), "50.0% all / 0.0% one hard");
    }

    #[test]
    fn memory_picks_the_largest_exact_unit() {
        assert_eq!(memory(Some(2 * GIB)), "2g");
        assert_eq!(memory(Some(512 * MIB)), "512m");
        assert_eq!(memory(Some(GIB + GIB / 2)), "1.5g");
        assert_eq!(memory(Some(4096)), "4096b");
        assert_eq!(memory(Some(MIB + MIB / 2)), "1.5m");
        assert_eq!(memory(None), "-");
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
        assert_eq!(priority(None), "-");
        assert_eq!(priority(Some(0x40)), "idle");
        assert_eq!(priority(Some(0x4000)), "belownormal");
        assert_eq!(priority(Some(0x20)), "normal");
        assert_eq!(priority(Some(0x8000)), "abovenormal");
        assert_eq!(priority(Some(0x80)), "high");
        assert_eq!(priority(Some(0x100)), "realtime");
        assert_eq!(priority(Some(0x200)), "0x200");
    }

    #[test]
    fn affinity_is_hex() {
        assert_eq!(affinity(None), "-");
        assert_eq!(affinity(Some(0xffff)), "0xffff");
        assert_eq!(affinity(Some(0)), "0x0");
    }

    #[test]
    fn chain_reports_membership_and_caps() {
        let jobs = vec![job(1, None), job(2, cap(1000, true))];
        assert_eq!(chain(&proc(1, vec![]), &jobs, 16), "NOT-IN-WNICE-JOB");
        // Unresolvable ids read as "not in a job".
        assert_eq!(chain(&proc(1, vec![9]), &jobs, 16), "NOT-IN-WNICE-JOB");
        assert_eq!(chain(&proc(1, vec![1]), &jobs, 16), "1 (none) UNCAPPED");
        assert_eq!(
            chain(&proc(1, vec![2]), &jobs, 16),
            "2 (10.0% all / 160.0% one hard)"
        );
        assert_eq!(
            chain(&proc(1, vec![1, 2]), &jobs, 16),
            "1 (none), 2 (10.0% all / 160.0% one hard)"
        );
    }

    fn scan() -> Scan {
        Scan {
            jobs: vec![
                JobInfo {
                    id: 1,
                    owner_pid: 12345,
                    owner_image: "wnice.exe".to_string(),
                    owner_command: Some("wnice -c 20 -- npm run build".to_string()),
                    cpu: cap(2000, true),
                    job_memory: Some(512 * MIB),
                    active_process_limit: Some(10),
                    affinity: Some(0xffff),
                    priority: Some(0x4000),
                    active_processes: 3,
                },
                JobInfo {
                    id: 2,
                    owner_pid: 23456,
                    owner_image: "capc.exe".to_string(),
                    owner_command: Some("capc 10 -- node server.js".to_string()),
                    cpu: None,
                    job_memory: None,
                    active_process_limit: None,
                    affinity: None,
                    priority: None,
                    active_processes: 1,
                },
            ],
            processes: vec![
                ProcInfo {
                    pid: 34567,
                    image: "node.exe".to_string(),
                    cpu_seconds: 0.3,
                    jobs: vec![1],
                },
                ProcInfo {
                    pid: 40000,
                    image: "svchost.exe".to_string(),
                    cpu_seconds: 0.1,
                    jobs: vec![],
                },
            ],
            uninspected_owners: 0,
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
2    23456 capc.exe              capc 10 -- node server.js           none                                -        -      -          -            1

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
        let text = render(&Scan::default());
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
        s.uninspected_owners = 2;
        s.jobs.clear();
        s.processes.clear();
        let text = render(&s);
        assert!(text
            .ends_with("owner processes not inspected: 2 (protected, elevated or another user)\n"));
    }
}
