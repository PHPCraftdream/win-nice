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

/// "Is the owner process at `owner_pos` a member of the job at `job_pos`?"
/// Positions index the owned-job list; `job_pos + 1` is that job's row id.
trait OwnerMembership {
    fn owner_in_job(&self, owner_pos: usize, job_pos: usize) -> Option<bool>;

    /// Position of the owner process of the job at `job_pos` in the owner
    /// handle list; identity for a seam whose positions already coincide.
    fn owner_pos_of(&self, job_pos: usize) -> usize {
        job_pos
    }
}

/// The real seam: the retained owner handles and the owned job handles.
struct RetainedJobs<'a> {
    owners: &'a [Option<Handle>],
    jobs: &'a [OwnedJob],
}

impl OwnerMembership for RetainedJobs<'_> {
    fn owner_in_job(&self, owner_pos: usize, job_pos: usize) -> Option<bool> {
        let owner = self.owners.get(owner_pos)?.as_ref()?;
        win::is_in_job(owner, &self.jobs.get(job_pos)?.handle)
    }

    fn owner_pos_of(&self, job_pos: usize) -> usize {
        self.jobs
            .get(job_pos)
            .map_or(job_pos, |job| job.owner_index)
    }
}

/// The real inspector: no state, one snapshot per `Inspector::scan` call.
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
        // Discovery is what a definitive chain verdict needs in the first place.
        let discovery_ok = own_sid.is_some() && table.is_some() && job_type.is_some();

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
            complete: inventory_complete(discovery_ok, &uninspected),
            jobs,
            processes,
            uninspected,
            logical_cpus,
            window_ms,
        }
    }
}

/// Complete enough for a definitive chain verdict: discovery succeeded (own
/// SID, handle table, job type) and every same-user owner was inspected.
pub(crate) fn inventory_complete(discovery_ok: bool, failures: &[OwnerFailure]) -> bool {
    discovery_ok
        && failures
            .iter()
            .all(|f| f.reason == Uninspected::DifferentUser)
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
    // to it: the first owner that succeeds in duplicating it claims it.
    let mut claimed: HashSet<usize> = HashSet::new();
    // Objects this owner could not duplicate: a later handle for the same
    // object may still succeed, so only the leftovers count as a failure.
    let mut unduplicated: HashSet<usize> = HashSet::new();

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
        // SID gate first: nothing else is ever asked of a foreign process.
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
        let command = win::command_line(&owner);

        for entry in table
            .entries_of(*pid)
            .into_iter()
            .filter(|e| e.object_type_index == job_type)
        {
            if claimed.contains(&entry.object) {
                continue;
            }
            let Some(handle) = win::duplicate_job(&owner, entry.handle_value) else {
                unduplicated.insert(entry.object);
                continue;
            };
            unduplicated.remove(&entry.object);
            claimed.insert(entry.object);
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
        if !unduplicated.is_empty() {
            failures.push(fail(Uninspected::JobQuery));
        }
        // Per owner: the retry for the same object happens inside this owner's
        // entry loop above, and a failed duplicate never claims the address.
        unduplicated.clear();
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
    // job's own owner about every other job keeps the rule easy to audit.
    let count = owned.len();
    let members = RetainedJobs {
        owners: owner_handles,
        jobs: &owned,
    };
    let nestings: Vec<Nesting> = (0..count)
        .map(|own_pos| classify_nesting(own_pos, count, &members))
        .collect();
    for (job, nesting) in owned.iter_mut().zip(nestings) {
        job.info.nesting = nesting;
    }
    owned
}

/// Nesting of the job at `own_pos`: candidates are the jobs whose member this
/// job's OWN owner process is; the immediate parent is the deepest candidate,
/// i.e. the one that is itself a member of every other candidate.
fn classify_nesting(own_pos: usize, count: usize, members: &dyn OwnerMembership) -> Nesting {
    let own_owner = members.owner_pos_of(own_pos);
    let mut candidates: Vec<usize> = Vec::new();
    let mut any_unknown = false;
    for k in 0..count {
        if k == own_pos {
            continue;
        }
        match members.owner_in_job(own_owner, k) {
            Some(true) => candidates.push(k),
            Some(false) => {}
            None => any_unknown = true,
        }
    }
    if candidates.is_empty() {
        return if any_unknown {
            Nesting::Unknown
        } else {
            Nesting::TopLevel
        };
    }
    // Depth of a candidate: how many other candidates contain its owner, plus
    // how many of those questions went unanswered.
    let mut depths: Vec<(usize, usize, usize)> = Vec::new();
    for &k in &candidates {
        let owner = members.owner_pos_of(k);
        let mut score = 0usize;
        let mut unknown = 0usize;
        for &l in &candidates {
            if l == k {
                continue;
            }
            match members.owner_in_job(owner, l) {
                Some(true) => score += 1,
                Some(false) => {}
                None => unknown += 1,
            }
        }
        depths.push((k, score, unknown));
    }
    // Only a strictly deepest, fully known candidate names the parent: an
    // ambiguous or unknowable depth must not name one.
    let Some(deepest) = depths.iter().map(|&(_, score, _)| score).max() else {
        return Nesting::Unknown;
    };
    let mut winners = depths.iter().filter(|&&(_, score, _)| score == deepest);
    match (winners.next(), winners.next()) {
        (Some(&(k, _, 0)), None) => Nesting::Nested(k + 1),
        _ => Nesting::Unknown,
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
    use std::cell::RefCell;

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

    fn owner_failure(pid: u32, reason: Uninspected) -> OwnerFailure {
        OwnerFailure {
            pid,
            image: "capc.exe".to_string(),
            reason,
        }
    }

    #[test]
    fn inventory_is_complete_only_with_discovery_and_no_same_user_failure() {
        // Discovery failure: no verdict is possible at all.
        assert!(!inventory_complete(false, &[]));
        assert!(!inventory_complete(
            false,
            &[owner_failure(1, Uninspected::DifferentUser)]
        ));
        // Everything discovered, nothing failed.
        assert!(inventory_complete(true, &[]));
        // A same-user owner we could not read breaks the inventory.
        assert!(!inventory_complete(
            true,
            &[owner_failure(1, Uninspected::AccessDenied)]
        ));
        assert!(!inventory_complete(
            true,
            &[owner_failure(1, Uninspected::JobQuery)]
        ));
        // Foreign owners are not part of our inventory.
        assert!(inventory_complete(
            true,
            &[owner_failure(1, Uninspected::DifferentUser)]
        ));
        assert!(inventory_complete(
            true,
            &[
                owner_failure(1, Uninspected::DifferentUser),
                owner_failure(2, Uninspected::DifferentUser),
            ]
        ));
        // ... but one real failure among them still is.
        assert!(!inventory_complete(
            true,
            &[
                owner_failure(1, Uninspected::DifferentUser),
                owner_failure(2, Uninspected::TokenQuery),
            ]
        ));
    }

    /// One owned job at a fixed position, with a handle the fake seam never
    /// touches (so no real kernel object is involved).
    fn owned_job(pos: usize, owner_index: usize) -> OwnedJob {
        OwnedJob {
            handle: Handle::from_raw_no_close(std::ptr::null_mut()),
            object: pos,
            owner_index,
            info: JobInfo {
                id: pos + 1,
                owner_pid: pos as u32,
                owner_image: "wnice.exe".to_string(),
                owner_command: None,
                cpu: crate::front::jobs::CapState::Unknown,
                limits: None,
                active_processes: None,
                nesting: Nesting::TopLevel,
            },
        }
    }

    /// Owned jobs whose owner positions deliberately differ from their own.
    fn owned_jobs(owner_indexes: &[usize]) -> Vec<OwnedJob> {
        owner_indexes
            .iter()
            .enumerate()
            .map(|(pos, &owner_index)| owned_job(pos, owner_index))
            .collect()
    }

    /// Seam that answers the listed pairs (`Some(false)` when unlisted) and
    /// records every pair it was asked.
    struct FakeMembership {
        owner_pos: Vec<usize>,
        answers: Vec<(usize, usize, Option<bool>)>,
        asked: RefCell<Vec<(usize, usize)>>,
    }

    impl FakeMembership {
        fn new(jobs: &[OwnedJob], answers: &[(usize, usize, Option<bool>)]) -> FakeMembership {
            FakeMembership {
                owner_pos: jobs.iter().map(|j| j.owner_index).collect(),
                answers: answers.to_vec(),
                asked: RefCell::new(Vec::new()),
            }
        }

        fn asked(&self, owner_pos: usize, job_pos: usize) -> bool {
            self.asked.borrow().contains(&(owner_pos, job_pos))
        }
    }

    impl OwnerMembership for FakeMembership {
        fn owner_in_job(&self, owner_pos: usize, job_pos: usize) -> Option<bool> {
            self.asked.borrow_mut().push((owner_pos, job_pos));
            self.answers
                .iter()
                .find(|(o, j, _)| *o == owner_pos && *j == job_pos)
                .map_or(Some(false), |&(_, _, answer)| answer)
        }

        fn owner_pos_of(&self, job_pos: usize) -> usize {
            self.owner_pos[job_pos]
        }
    }

    #[test]
    fn nesting_asks_about_the_own_owner_of_the_classified_job() {
        // Job 0's owner sits at position 1, job 1's owner at position 2.
        let jobs = owned_jobs(&[1, 2]);
        let fake = FakeMembership::new(&jobs, &[(2, 0, Some(true))]);
        assert_eq!(classify_nesting(1, jobs.len(), &fake), Nesting::Nested(1));
        assert_eq!(classify_nesting(0, jobs.len(), &fake), Nesting::TopLevel);
        // The question was about job 1's own owner (position 2), not about
        // the owner of the job it is nested in.
        assert!(fake.asked(2, 0));
        assert!(!fake.asked(2, 1));
    }

    #[test]
    fn nesting_names_the_immediate_parent() {
        // Job 2's owner is in jobs 1 and 0; job 1's owner is in job 0.
        let jobs = owned_jobs(&[3, 4, 5]);
        let fake = FakeMembership::new(
            &jobs,
            &[(5, 0, Some(true)), (5, 1, Some(true)), (4, 0, Some(true))],
        );
        assert_eq!(classify_nesting(2, jobs.len(), &fake), Nesting::Nested(2));
        assert_eq!(classify_nesting(1, jobs.len(), &fake), Nesting::Nested(1));
        assert_eq!(classify_nesting(0, jobs.len(), &fake), Nesting::TopLevel);
    }

    #[test]
    fn nesting_is_top_level_without_a_candidate_and_unknown_without_answers() {
        let jobs = owned_jobs(&[0, 1]);
        // No answer was true.
        assert_eq!(
            classify_nesting(1, jobs.len(), &FakeMembership::new(&jobs, &[])),
            Nesting::TopLevel
        );
        // An unanswered pair and no candidate: no verdict.
        assert_eq!(
            classify_nesting(1, jobs.len(), &FakeMembership::new(&jobs, &[(1, 0, None)])),
            Nesting::Unknown
        );
        // A candidate plus an unanswered pair is still a nesting.
        let three = owned_jobs(&[0, 1, 2]);
        let fake = FakeMembership::new(&three, &[(1, 0, Some(true)), (1, 2, None)]);
        assert_eq!(classify_nesting(1, three.len(), &fake), Nesting::Nested(1));
    }

    #[test]
    fn nesting_is_unknown_when_two_siblings_are_equally_deep() {
        // Job 2's owner is in both siblings, and neither contains the other:
        // there is no unique immediate parent.
        let jobs = owned_jobs(&[0, 1, 2]);
        let fake = FakeMembership::new(&jobs, &[(2, 0, Some(true)), (2, 1, Some(true))]);
        assert_eq!(classify_nesting(2, jobs.len(), &fake), Nesting::Unknown);
    }

    #[test]
    fn nesting_is_unknown_when_a_candidate_depth_is_unanswered() {
        // Two candidates, and whether the deeper one contains the other is
        // unknown: naming either would be a guess.
        let jobs = owned_jobs(&[0, 1, 2]);
        let fake = FakeMembership::new(
            &jobs,
            &[(2, 0, Some(true)), (2, 1, Some(true)), (1, 0, None)],
        );
        assert_eq!(classify_nesting(2, jobs.len(), &fake), Nesting::Unknown);
    }
}
