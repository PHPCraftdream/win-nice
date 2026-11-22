//! Read-only inspection of the Job Objects that wnice-family wrappers hold.
//!
//! Backs the `wnice --jobs` report: it enumerates the wrapper processes, finds
//! the job handles they own, reads the limits those jobs carry and samples the
//! CPU time of the busiest processes. Nothing here mutates a job or a process.

pub mod format;
pub mod inspect;
pub mod win;

/// CPU sampling window for the process table.
pub const SAMPLE_WINDOW: std::time::Duration = std::time::Duration::from_millis(250);
/// Rows in the process table.
pub const TOP_PROCESSES: usize = 10;

/// CPU rate cap read from `JOBOBJECT_CPU_RATE_CONTROL_INFORMATION`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuCap {
    /// Percent of all cores times 100 (10000 == 100%).
    pub cpu_rate: u32,
    /// Hard cap (`JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP`) instead of a soft one.
    pub hard: bool,
}

/// One Job Object held by a wrapper process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobInfo {
    /// 1-based row id, referenced by [`ProcInfo::jobs`].
    pub id: usize,
    pub owner_pid: u32,
    /// Image name of the owning wrapper, e.g. `"wnice.exe"`.
    pub owner_image: String,
    /// The wrapper's command line; `None` when it could not be read.
    pub owner_command: Option<String>,
    pub cpu: Option<CpuCap>,
    /// `JOB_OBJECT_LIMIT_JOB_MEMORY` only.
    pub job_memory: Option<u64>,
    /// `JOB_OBJECT_LIMIT_ACTIVE_PROCESS` only.
    pub active_process_limit: Option<u32>,
    /// `JOB_OBJECT_LIMIT_AFFINITY` only.
    pub affinity: Option<u64>,
    /// `JOB_OBJECT_LIMIT_PRIORITY_CLASS` only.
    pub priority: Option<u32>,
    /// `JobObjectBasicAccountingInformation::ActiveProcesses`.
    pub active_processes: u32,
}

/// One sampled process.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcInfo {
    pub pid: u32,
    pub image: String,
    /// CPU consumed during the sampling window.
    pub cpu_seconds: f64,
    /// [`JobInfo`] ids containing this process, ascending.
    pub jobs: Vec<usize>,
}

/// One inspection pass.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scan {
    /// Display order, ids `1..=len`.
    pub jobs: Vec<JobInfo>,
    /// Busiest first, at most [`TOP_PROCESSES`].
    pub processes: Vec<ProcInfo>,
    pub uninspected_owners: usize,
    pub logical_cpus: u32,
    pub window_ms: u64,
}

/// Seam over the inspection primitives so tests can feed fake data.
pub trait Inspector {
    fn scan(&self) -> Scan;
}

/// Entry point for `wnice --jobs`; always returns 0.
pub fn run() -> i32 {
    run_to(&inspect::RealInspector, &mut std::io::stdout())
}

/// Same as [`run`], writing to `out` (testable). Write errors are ignored.
pub fn run_to(inspector: &dyn Inspector, out: &mut dyn std::io::Write) -> i32 {
    let scan = inspector.scan();
    let _ = out.write_all(format::render(&scan).as_bytes());
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeInspector(Scan);

    impl Inspector for FakeInspector {
        fn scan(&self) -> Scan {
            self.0.clone()
        }
    }

    fn fake_scan() -> Scan {
        Scan {
            jobs: vec![JobInfo {
                id: 1,
                owner_pid: 4242,
                owner_image: "wnice.exe".to_string(),
                owner_command: Some("wnice -c 50 -- cmd".to_string()),
                cpu: Some(CpuCap {
                    cpu_rate: 5000,
                    hard: true,
                }),
                job_memory: None,
                active_process_limit: None,
                affinity: None,
                priority: None,
                active_processes: 2,
            }],
            processes: vec![ProcInfo {
                pid: 4343,
                image: "cmd.exe".to_string(),
                cpu_seconds: 0.1,
                jobs: vec![1],
            }],
            uninspected_owners: 1,
            logical_cpus: 8,
            window_ms: 250,
        }
    }

    #[test]
    fn run_to_writes_the_tables_and_returns_zero() {
        let scan = fake_scan();
        let mut out: Vec<u8> = Vec::new();
        let code = run_to(&FakeInspector(scan.clone()), &mut out);
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8(out).unwrap(), format::render(&scan));
        let text = format::render(&scan);
        assert!(text.contains("JOBS: 1 job object(s)"));
        assert!(text.contains("owner processes not inspected: 1 (protected"));
    }

    #[test]
    fn run_to_is_zero_for_an_empty_scan() {
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(run_to(&FakeInspector(Scan::default()), &mut out), 0);
        assert!(String::from_utf8(out)
            .unwrap()
            .contains("JOBS: 0 job object(s)"));
    }
}
