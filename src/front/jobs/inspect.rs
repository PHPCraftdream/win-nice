//! The real (Win32-backed) inspection pass behind `wnice --jobs`.

use super::win::{self, HandleTable};
use super::{
    JobInfo, Nesting, OwnerFailure, ProcInfo, Scan, Uninspected, SAMPLE_WINDOW, TOP_PROCESSES,
};
use crate::Handle;
use std::collections::HashSet;

/// Multicall frontend names (image stems) that hold a wrapped command's job.
const WRAPPER_NAMES: [&str; 15] = [
    "wnice",
    "idle",
    "belownormal",
    "abovenormal",
    "high",
    "realtime",
    "capc",
    "capt",
    "capm",
    "caps",
    "capn",
    "cy",
    "cx",
    "admin",
    "uiup",
];

/// True when `image` (a bare file name such as `"wnice.exe"`) is one of the
/// wnice-family multicall frontends: the stem is lowercased and `.exe` stripped.
pub fn is_wrapper_image(image: &str) -> bool {
    let lower = image.to_ascii_lowercase();
    let stem = lower.strip_suffix(".exe").unwrap_or(&lower);
    WRAPPER_NAMES.contains(&stem)
}

/// A duplicated job handle kept alive together with its row, so the nesting
/// pass can query membership without duplicating it again.
struct OwnedJob {
    handle: Handle,
    /// Kernel job-object address: the identity shared by several handles.
    object: usize,
    /// Position of the owning process in the recognised-owner list.
    owner_index: usize,
    info: JobInfo,
}

/// The real inspector: no state, one snapshot per [`Inspector::scan`] call.
pub struct RealInspector;

impl super::Inspector for RealInspector {
    fn scan(&self) -> Scan {
        let logical_cpus = win::logical_cpus();
        let window_ms = SAMPLE_WINDOW.as_millis() as u64;
        let me = std::process::id();

        // The handle table is the only way to reach another process's job
        // handles; the job type index is resolved at runtime from a probe job.
        let table = HandleTable::query();
        let job_type = table.as_ref().and_then(win::job_type_index);
        let own_sid = win::current_user();

        let processes = crate::front::native::processes();
        let owners = recognised_owners(&processes, me);

        // A failure that precedes every per-owner step leaves no owner
        // inspected at all, but never stops the sampling pass.
        let (owned, owner_handles, uninspected) = match (&own_sid, &table, job_type) {
            (None, ..) => (
                Vec::new(),
                Vec::new(),
                all_uninspected(&owners, Uninspected::TokenQuery),
            ),
            (Some(_), None, _) => (
                Vec::new(),
                Vec::new(),
                all_uninspected(&owners, Uninspected::HandleTable),
            ),
            (Some(_), Some(_), None) => (
                Vec::new(),
                Vec::new(),
                all_uninspected(&owners, Uninspected::JobType),
            ),
            (Some(sid), Some(table), Some(job_type)) => {
                inspect_owners(sid, table, job_type, &owners)
            }
        };

        let owned = with_nesting(owned, &owner_handles);
        let jobs: Vec<JobInfo> = owned.iter().map(|j| j.info.clone()).collect();
        let processes = sample(&processes, me, &owned);

        Scan {
            jobs,
            processes,
            uninspected,
            logical_cpus,
            window_ms,
        }
    }
}

/// The wrapper processes worth inspecting, pid-ascending: this order decides
/// which owner claims a job object that several wrappers share.
fn recognised_owners(processes: &[(String, u32)], me: u32) -> Vec<(u32, String)> {
    let mut owners: Vec<(u32, String)> = processes
        .iter()
        .filter(|(image, pid)| *pid != me && is_wrapper_image(image))
        .map(|(image, pid)| (*pid, image.clone()))
        .collect();
    owners.sort_by_key(|(pid, _)| *pid);
    owners
}

/// Every owner counted as not inspected, in the given order (used when the
/// handle snapshot or the job type index is unavailable).
pub(crate) fn all_uninspected(owners: &[(u32, String)], reason: Uninspected) -> Vec<OwnerFailure> {
    owners
        .iter()
        .map(|(pid, image)| OwnerFailure {
            pid: *pid,
            image: image.clone(),
            reason,
        })
        .collect()
}

/// Open each owner, keep its job handles and collect the ones that failed.
/// Returns the jobs, one owner handle per owner position (`None` when the
/// process could not be opened) and the failures in owner order.
fn inspect_owners(
    own: &win::OwnedSid,
    table: &HandleTable,
    job_type: u16,
    owners: &[(u32, String)],
) -> (Vec<OwnedJob>, Vec<Option<Handle>>, Vec<OwnerFailure>) {
    let mut owned: Vec<OwnedJob> = Vec::new();
    let mut owner_handles: Vec<Option<Handle>> = owners.iter().map(|_| None).collect();
    let mut failures: Vec<OwnerFailure> = Vec::new();
    // One row per kernel job object, even when a wrapper holds several handles
    // to it: the first owner that sees an address claims it.
    let mut claimed: HashSet<usize> = HashSet::new();

    for (index, (pid, image)) in owners.iter().enumerate() {
        let fail = |reason| OwnerFailure {
            pid: *pid,
            image: image.clone(),
            reason,
        };
        let Some(owner) = win::open_owner(*pid) else {
            failures.push(fail(Uninspected::AccessDenied));
            continue;
        };
        let command = win::command_line(&owner);
        match win::same_user(&owner, own) {
            Some(false) => {
                failures.push(fail(Uninspected::DifferentUser));
                continue;
            }
            None => {
                failures.push(fail(Uninspected::TokenQuery));
                continue;
            }
            Some(true) => {}
        }

        let mut failed = false;
        for entry in table
            .entries_of(*pid)
            .into_iter()
            .filter(|e| e.object_type_index == job_type)
        {
            if !claimed.insert(entry.object) {
                continue;
            }
            let Some(handle) = win::duplicate_job(&owner, entry.handle_value) else {
                failed = true;
                continue;
            };
            let limits = win::job_limits(&handle);
            owned.push(OwnedJob {
                handle,
                object: entry.object,
                owner_index: index,
                info: JobInfo {
                    id: 0,
                    owner_pid: *pid,
                    owner_image: image.clone(),
                    owner_command: command.clone(),
                    cpu: limits.cpu,
                    limits: limits.extended,
                    active_processes: limits.active,
                    nesting: Nesting::TopLevel,
                },
            });
        }
        if failed {
            failures.push(fail(Uninspected::JobQuery));
        }
        owner_handles[index] = Some(owner);
    }

    (owned, owner_handles, failures)
}

/// Number the rows in a deterministic order and classify each one's nesting.
fn with_nesting(mut owned: Vec<OwnedJob>, owner_handles: &[Option<Handle>]) -> Vec<OwnedJob> {
    owned.sort_by_key(|j| (j.info.owner_pid, j.object));
    for (i, job) in owned.iter_mut().enumerate() {
        job.info.id = i + 1;
    }
    // O(n^2) on purpose: the job list is a handful of rows, and asking each
    // owner process about every other job keeps the rule easy to audit.
    let nestings: Vec<Nesting> = (0..owned.len())
        .map(|own| {
            let others: Vec<(usize, Option<bool>)> = owned
                .iter()
                .filter(|other| other.info.id != owned[own].info.id)
                .map(|other| {
                    let member = owner_handles[other.owner_index]
                        .as_ref()
                        .and_then(|handle| win::is_in_job(handle, &other.handle));
                    (other.info.id, member)
                })
                .collect();
            classify_nesting(owned[own].info.id, &others)
        })
        .collect();
    for (job, nesting) in owned.iter_mut().zip(nestings) {
        job.info.nesting = nesting;
    }
    owned
}

/// Nesting of the job with row id `own` given the membership of its owner
/// process in every OTHER scanned job (`(row id, Some(true/false))`, `None` =
/// that query failed): the smallest id that contains it, else `Unknown` if any
/// answer was unknown, else `TopLevel`.
pub(crate) fn classify_nesting(own: usize, others: &[(usize, Option<bool>)]) -> Nesting {
    let mut smallest: Option<usize> = None;
    let mut unknown = false;
    for &(id, answer) in others {
        if id == own {
            continue;
        }
        match answer {
            Some(true) => smallest = Some(smallest.map_or(id, |best: usize| best.min(id))),
            Some(false) => {}
            None => unknown = true,
        }
    }
    match smallest {
        Some(parent) => Nesting::Nested(parent),
        None if unknown => Nesting::Unknown,
        None => Nesting::TopLevel,
    }
}

/// Two CPU-time samples around the window, then membership of the busiest
/// processes. The handle taken for the first sample is reused for both the
/// second sample and the membership queries, so every reading describes the
/// same process.
fn sample(processes: &[(String, u32)], me: u32, owned: &[OwnedJob]) -> Vec<ProcInfo> {
    let mut first: Vec<(u32, String, Handle, u64)> = Vec::new();
    for (image, pid) in processes {
        if *pid == me {
            continue;
        }
        let Some(handle) = win::open_query(*pid) else {
            continue;
        };
        let Some(start) = win::cpu_time_100ns(&handle) else {
            continue;
        };
        first.push((*pid, image.clone(), handle, start));
    }
    std::thread::sleep(SAMPLE_WINDOW);

    let mut samples: Vec<(u32, String, Handle, f64)> = Vec::new();
    for (pid, image, handle, start) in first {
        let Some(end) = win::cpu_time_100ns(&handle) else {
            continue;
        };
        let delta = end.saturating_sub(start) as f64 * 1e-7;
        if delta > 0.0 {
            samples.push((pid, image, handle, delta));
        }
    }
    // Busiest first, pid ascending to break ties.
    samples.sort_by(|a, b| b.3.total_cmp(&a.3).then(a.0.cmp(&b.0)));
    samples.truncate(TOP_PROCESSES);

    samples
        .into_iter()
        .map(|(pid, image, handle, cpu_seconds)| ProcInfo {
            pid,
            image,
            cpu_seconds,
            jobs: memberships(&handle, owned),
        })
        .collect()
}

/// The job ids containing the sampled process; `None` when any query failed.
fn memberships(proc: &Handle, owned: &[OwnedJob]) -> Option<Vec<usize>> {
    let mut ids: Vec<usize> = Vec::new();
    let mut unknown = false;
    for job in owned {
        match win::is_in_job(proc, &job.handle) {
            Some(true) => ids.push(job.info.id),
            Some(false) => {}
            None => unknown = true,
        }
    }
    if unknown {
        return None;
    }
    ids.sort_unstable();
    ids.dedup();
    Some(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapper_images_are_the_multicall_names() {
        assert!(is_wrapper_image("wnice.exe"));
        assert!(is_wrapper_image("CAPC.EXE"));
        assert!(is_wrapper_image("capm"));
        assert!(is_wrapper_image("Admin.exe"));
        assert!(is_wrapper_image("uiup.exe"));
        assert!(!is_wrapper_image("node.exe"));
        assert!(!is_wrapper_image("wnice.cmd"));
        assert!(!is_wrapper_image("wnice2.exe"));
        assert!(!is_wrapper_image(""));
    }

    #[test]
    fn all_uninspected_keeps_every_owner_in_order() {
        let owners = vec![
            (300u32, "wnice.exe".to_string()),
            (100, "capc.exe".to_string()),
        ];
        let want = vec![
            OwnerFailure {
                pid: 300,
                image: "wnice.exe".to_string(),
                reason: Uninspected::JobType,
            },
            OwnerFailure {
                pid: 100,
                image: "capc.exe".to_string(),
                reason: Uninspected::JobType,
            },
        ];
        assert_eq!(all_uninspected(&owners, Uninspected::JobType), want);
        assert!(all_uninspected(&[], Uninspected::HandleTable).is_empty());
    }

    #[test]
    fn nesting_is_top_level_when_no_other_job_contains_it() {
        assert_eq!(classify_nesting(1, &[]), Nesting::TopLevel);
        assert_eq!(
            classify_nesting(1, &[(2, Some(false)), (3, Some(false))]),
            Nesting::TopLevel
        );
    }

    #[test]
    fn nesting_is_the_smallest_containing_id() {
        assert_eq!(
            classify_nesting(3, &[(5, Some(true)), (2, Some(true)), (4, Some(false))]),
            Nesting::Nested(2)
        );
        assert_eq!(
            classify_nesting(1, &[(9, Some(true)), (8, None)]),
            Nesting::Nested(9)
        );
    }

    #[test]
    fn nesting_is_unknown_only_when_no_answer_was_true() {
        assert_eq!(
            classify_nesting(1, &[(2, None), (3, Some(false))]),
            Nesting::Unknown
        );
        // A failed query never overrides a positive containment answer.
        assert_eq!(
            classify_nesting(1, &[(2, None), (3, Some(true))]),
            Nesting::Nested(3)
        );
    }
}
