//! Read-only inspection of the Job Objects that wnice-family wrappers hold.
//!
//! Backs the `wnice --jobs` report: it enumerates the wrapper processes, finds
//! the job handles they own, reads the limits those jobs carry and samples the
//! CPU time of the busiest processes. Nothing here mutates a job or a process.
//!
//! Every questioned value has an explicit unknown state: a failed query is
//! reported as unknown, never as "no limit" or "no job".

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
    /// `CpuRate` as the kernel stores it: percent * 100 (10000 == 100%),
    /// relative to the whole machine for a top-level job and to the parent
    /// job's CPU allowance for a nested one.
    pub cpu_rate: u32,
    /// Hard cap (`JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP`) instead of a soft one.
    pub hard: bool,
}

/// A job's CPU cap: known-absent, known, or not known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapState {
    Unknown,
    Uncapped,
    Capped(CpuCap),
}

/// `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` limits; each `None` member means "that
/// limit is not set". The whole struct is absent when the query failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LimitFields {
    /// `JOB_OBJECT_LIMIT_JOB_MEMORY` only.
    pub job_memory: Option<u64>,
    /// `JOB_OBJECT_LIMIT_ACTIVE_PROCESS` only.
    pub active_process_limit: Option<u32>,
    /// `JOB_OBJECT_LIMIT_AFFINITY` only.
    pub affinity: Option<u64>,
    /// `JOB_OBJECT_LIMIT_PRIORITY_CLASS` only.
    pub priority: Option<u32>,
}

/// Where a job sits relative to the other scanned jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nesting {
    TopLevel,
    /// Row id of the scanned job whose member this job's owner process is.
    Nested(usize),
    /// Membership of the owner process could not be determined.
    Unknown,
}

/// One Job Object held by a wrapper process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobInfo {
    /// 1-based row id, referenced by `ProcInfo::jobs` and `Nesting::Nested`.
    pub id: usize,
    pub owner_pid: u32,
    /// Image name of the owning wrapper, e.g. `"wnice.exe"`.
    pub owner_image: String,
    /// The wrapper's command line; `None` when it could not be read.
    pub owner_command: Option<String>,
    pub cpu: CapState,
    /// `None` when the extended-limit query failed.
    pub limits: Option<LimitFields>,
    /// `JobObjectBasicAccountingInformation::ActiveProcesses`; `None` on failure.
    pub active_processes: Option<u32>,
    pub nesting: Nesting,
}

/// One sampled process.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcInfo {
    pub pid: u32,
    pub image: String,
    /// CPU consumed during the sampling window.
    pub cpu_seconds: f64,
    /// Unique ascending job row ids, or `None` when a membership query failed
    /// (no `UNCAPPED` / `NOT-IN-WNICE-JOB` verdict is then possible).
    pub jobs: Option<Vec<usize>>,
}

/// Why one owner process could not be inspected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Uninspected {
    DifferentUser,
    AccessDenied,
    TokenQuery,
    HandleTable,
    JobType,
    JobQuery,
}

impl Uninspected {
    /// Short reason shown in the footer.
    pub fn label(self) -> &'static str {
        match self {
            Uninspected::DifferentUser => "different user",
            Uninspected::AccessDenied => "access denied",
            Uninspected::TokenQuery => "token query failed",
            Uninspected::HandleTable => "handle table unavailable",
            Uninspected::JobType => "job type unavailable",
            Uninspected::JobQuery => "job query failed",
        }
    }
}

/// One owner process that could not be inspected, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerFailure {
    pub pid: u32,
    pub image: String,
    pub reason: Uninspected,
}

/// One inspection pass.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scan {
    /// False when part of the current user's job inventory could not be read,
    /// so a missing or cap-free chain is not a verdict.
    pub complete: bool,
    /// Display order, ids `1..=len`.
    pub jobs: Vec<JobInfo>,
    /// Busiest first, at most `TOP_PROCESSES`.
    pub processes: Vec<ProcInfo>,
    /// Owners that produced no rows, in owner order.
    pub uninspected: Vec<OwnerFailure>,
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

/// Same as `run`, writing to `out` (testable). Write errors are ignored.
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
            complete: true,
            jobs: vec![JobInfo {
                id: 1,
                owner_pid: 4242,
                owner_image: "wnice.exe".to_string(),
                owner_command: Some("wnice -c 50 -- cmd".to_string()),
                cpu: CapState::Capped(CpuCap {
                    cpu_rate: 5000,
                    hard: true,
                }),
                limits: None,
                active_processes: Some(2),
                nesting: Nesting::TopLevel,
            }],
            processes: vec![ProcInfo {
                pid: 4343,
                image: "cmd.exe".to_string(),
                cpu_seconds: 0.1,
                jobs: Some(vec![1]),
            }],
            uninspected: vec![OwnerFailure {
                pid: 5000,
                image: "capc.exe".to_string(),
                reason: Uninspected::AccessDenied,
            }],
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
        assert!(text.contains("owner processes not inspected: 1 (1 access denied)"));
        assert!(text.contains("  not inspected: 5000 capc.exe (access denied)"));
    }

    #[test]
    fn run_to_is_zero_for_an_empty_scan() {
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(run_to(&FakeInspector(Scan::default()), &mut out), 0);
        assert!(String::from_utf8(out)
            .unwrap()
            .contains("JOBS: 0 job object(s)"));
    }

    #[test]
    fn run_to_names_every_uninspected_owner() {
        let scan = Scan {
            uninspected: vec![
                OwnerFailure {
                    pid: 11,
                    image: "wnice.exe".to_string(),
                    reason: Uninspected::DifferentUser,
                },
                OwnerFailure {
                    pid: 22,
                    image: "high.exe".to_string(),
                    reason: Uninspected::HandleTable,
                },
            ],
            complete: true,
            logical_cpus: 4,
            window_ms: 250,
            ..Scan::default()
        };
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(run_to(&FakeInspector(scan.clone()), &mut out), 0);
        assert_eq!(String::from_utf8(out).unwrap(), format::render(&scan));
        let text = format::render(&scan);
        assert!(text.contains("owner processes not inspected: 2 (1 different user, 1 handle table"));
        assert!(text.contains("JOBS: 0 job object(s)"));
    }
}
