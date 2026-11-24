//! The real (Win32-backed) inspection pass behind `wnice --jobs`.

use super::inventory::Inventory;
use super::win::{self, HandleTable};
use super::{JobInfo, Nesting, ProcInfo, Scan, Uninspected, SAMPLE_WINDOW, TOP_PROCESSES};
use crate::Handle;

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
        let mut inventory = Inventory::default();

        // The handle table is the only way to reach another process's job
        // handles; the job type index is resolved at runtime from a probe job.
        let table = HandleTable::query();
        let job_type = table.as_ref().and_then(win::job_type_index);
        let own_sid = win::current_user();

        // A failed snapshot is an incomplete inventory, never just no
        // processes: nothing could have been recognised either.
        let processes = match crate::front::native::process_snapshot_with_completeness() {
            Some((list, complete)) => {
                if !complete {
                    inventory.mark_incomplete();
                }
                list
            }
            None => {
                inventory.mark_incomplete();
                Vec::new()
            }
        };
        let owners = recognised_owners(&processes, me);

        // A failure that precedes every per-owner step leaves no owner
        // inspected at all, but never stops the sampling pass.
        let (owned, owner_handles) = match (&own_sid, &table, job_type) {
            (None, ..) => {
                inventory.discovery_failed(&owners, Uninspected::TokenQuery);
                (Vec::new(), Vec::new())
            }
            (Some(_), None, _) => {
                inventory.discovery_failed(&owners, Uninspected::HandleTable);
                (Vec::new(), Vec::new())
            }
            (Some(_), Some(_), None) => {
                inventory.discovery_failed(&owners, Uninspected::JobType);
                (Vec::new(), Vec::new())
            }
            (Some(sid), Some(table), Some(job_type)) => {
                inspect_owners(sid, table, job_type, &owners, &mut inventory)
            }
        };

        let owned = with_nesting(owned, &owner_handles, inventory.complete());
        let jobs: Vec<JobInfo> = owned.iter().map(|j| j.info.clone()).collect();
        let processes = sample(&processes, me, own_sid.as_ref(), &owned);
        let (complete, uninspected) = inventory.into_parts();

        Scan {
            complete,
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

/// Two phases over the owners, in the given (pid-ascending) order: first open
/// and screen every owner and collect its job handles as candidates, then try
/// to duplicate one handle per kernel job object. Returns the jobs and one
/// owner handle per owner position (`None` when the owner was not inspected).
fn inspect_owners(
    own: &win::OwnedSid,
    table: &HandleTable,
    job_type: u16,
    owners: &[(u32, String)],
    inventory: &mut Inventory,
) -> (Vec<OwnedJob>, Vec<Option<Handle>>) {
    let mut owner_handles: Vec<Option<Handle>> = owners.iter().map(|_| None).collect();
    let mut commands: Vec<Option<String>> = owners.iter().map(|_| None).collect();
    // Kernel job object address -> its candidate handles as (owner position,
    // handle value), in the order the owners and the table produced them.
    let mut objects: Vec<(usize, Vec<(usize, usize)>)> = Vec::new();

    for (index, (pid, image)) in owners.iter().enumerate() {
        let Some(owner) = win::open_owner(*pid) else {
            inventory.owner_failed(*pid, image, Uninspected::AccessDenied);
            continue;
        };
        // SID gate first: nothing else is ever asked of a foreign process.
        match win::same_user(&owner, own) {
            Some(true) => {}
            Some(false) => {
                inventory.owner_failed(*pid, image, Uninspected::DifferentUser);
                continue;
            }
            None => {
                inventory.owner_failed(*pid, image, Uninspected::TokenQuery);
                continue;
            }
        }
        commands[index] = win::command_line(&owner);
        for entry in table
            .entries_of(*pid)
            .into_iter()
            .filter(|e| e.object_type_index == job_type)
        {
            match objects
                .iter_mut()
                .find(|(object, _)| *object == entry.object)
            {
                Some((_, candidates)) => candidates.push((index, entry.handle_value)),
                None => objects.push((entry.object, vec![(index, entry.handle_value)])),
            }
        }
        owner_handles[index] = Some(owner);
    }

    // One row per kernel job object, even when several wrappers hold handles
    // to it: the first candidate handle that duplicates wins the object.
    let mut owned: Vec<OwnedJob> = Vec::new();
    for (object, candidates) in objects {
        let first_owner = candidates.first().map(|&(owner_index, _)| owner_index);
        let mut duplicated: Option<(Handle, usize)> = None;
        for (owner_index, handle_value) in candidates {
            // A candidate only exists for an owner whose handle was kept.
            let Some(owner) = owner_handles[owner_index].as_ref() else {
                continue;
            };
            if let Some(handle) = win::duplicate_job(owner, handle_value) {
                duplicated = Some((handle, owner_index));
                break;
            }
        }
        let Some((handle, owner_index)) = duplicated else {
            if let Some(first_owner) = first_owner {
                let (pid, image) = &owners[first_owner];
                inventory.job_failed(*pid, image);
            }
            continue;
        };
        let limits = win::job_limits(&handle);
        if !limits.queries_ok() {
            inventory.mark_incomplete();
        }
        let (pid, image) = &owners[owner_index];
        owned.push(OwnedJob {
            handle,
            object,
            owner_index,
            info: JobInfo {
                id: 0,
                owner_pid: *pid,
                owner_image: image.clone(),
                owner_command: commands[owner_index].clone(),
                cpu: limits.cpu,
                limits: limits.extended,
                active_processes: limits.active,
                nesting: Nesting::TopLevel,
            },
        });
    }

    (owned, owner_handles)
}

/// Number the rows in a deterministic order and classify each one's nesting.
fn with_nesting(
    mut owned: Vec<OwnedJob>,
    owner_handles: &[Option<Handle>],
    inventory_complete: bool,
) -> Vec<OwnedJob> {
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
        .map(|own_pos| classify_nesting(own_pos, count, &members, inventory_complete))
        .collect();
    for (job, nesting) in owned.iter_mut().zip(nestings) {
        job.info.nesting = nesting;
    }
    owned
}

/// Nesting of the job at `own_pos`: candidates are the jobs whose member this
/// job's OWN owner process is; the immediate parent is the deepest candidate,
/// i.e. the one that is itself a member of every other candidate. An incomplete
/// inventory can establish nesting but cannot prove that an intermediate owner
/// was not omitted.
fn classify_nesting(
    own_pos: usize,
    count: usize,
    members: &dyn OwnerMembership,
    inventory_complete: bool,
) -> Nesting {
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
    // A candidate plus an unanswered pair: the job is nested, but which job
    // contains it cannot be proven, so no depth is scored.
    if any_unknown {
        return Nesting::NestedUnnamed;
    }
    // The positive membership proves nesting, but an uninspected owner could
    // be an intermediate job, so incomplete inventories cannot name a parent.
    if !inventory_complete {
        return Nesting::NestedUnnamed;
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
    // ambiguous or unknowable depth leaves the nesting unnamed.
    let Some(deepest) = depths.iter().map(|&(_, score, _)| score).max() else {
        return Nesting::NestedUnnamed;
    };
    // A named parent must be contained by every other candidate. A merely
    // deepest candidate among incomparable/partially related candidates is not
    // enough to establish the immediate parent.
    if deepest != candidates.len().saturating_sub(1) {
        return Nesting::NestedUnnamed;
    }
    let mut winners = depths.iter().filter(|&&(_, score, _)| score == deepest);
    match (winners.next(), winners.next()) {
        (Some(&(k, _, 0)), None) => Nesting::Nested(k + 1),
        _ => Nesting::NestedUnnamed,
    }
}

/// Two CPU-time samples around the window, then membership of the busiest
/// processes. The handle taken for the first sample is reused for both the
/// second sample and the membership queries, so every reading describes the
/// same process. Only this user's processes are sampled at all.
fn sample(
    processes: &[(String, u32)],
    me: u32,
    own: Option<&win::OwnedSid>,
    owned: &[OwnedJob],
) -> Vec<ProcInfo> {
    // Without our own SID no process can be proven to be ours.
    let Some(own) = own else {
        return Vec::new();
    };
    let mut first: Vec<(u32, String, Handle, u64)> = Vec::new();
    for (image, pid) in processes {
        if *pid == me {
            continue;
        }
        let Some(handle) = win::open_query(*pid) else {
            continue;
        };
        // Same-user gate: a foreign process is never sampled, so it appears
        // nowhere except in the inventory failures.
        if win::same_user(&handle, own) != Some(true) {
            continue;
        }
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
        assert_eq!(
            classify_nesting(1, jobs.len(), &fake, true),
            Nesting::Nested(1)
        );
        assert_eq!(
            classify_nesting(0, jobs.len(), &fake, true),
            Nesting::TopLevel
        );
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
        assert_eq!(
            classify_nesting(2, jobs.len(), &fake, true),
            Nesting::Nested(2)
        );
        assert_eq!(
            classify_nesting(1, jobs.len(), &fake, true),
            Nesting::Nested(1)
        );
        assert_eq!(
            classify_nesting(0, jobs.len(), &fake, true),
            Nesting::TopLevel
        );
    }

    #[test]
    fn nesting_is_top_level_without_a_candidate_and_unknown_without_answers() {
        let jobs = owned_jobs(&[0, 1]);
        // No answer was true.
        assert_eq!(
            classify_nesting(1, jobs.len(), &FakeMembership::new(&jobs, &[]), true),
            Nesting::TopLevel
        );
        // An unanswered pair and no candidate: no verdict.
        assert_eq!(
            classify_nesting(
                1,
                jobs.len(),
                &FakeMembership::new(&jobs, &[(1, 0, None)]),
                true
            ),
            Nesting::Unknown
        );
        // A candidate plus an unanswered pair is a nesting with no provable
        // parent: the third job's answer is missing, so depth is not scored.
        let three = owned_jobs(&[0, 1, 2]);
        let fake = FakeMembership::new(&three, &[(1, 0, Some(true)), (1, 2, None)]);
        assert_eq!(
            classify_nesting(1, three.len(), &fake, true),
            Nesting::NestedUnnamed
        );
    }

    #[test]
    fn incomplete_inventory_keeps_known_nesting_unnamed() {
        let jobs = owned_jobs(&[0, 1]);
        let fake = FakeMembership::new(&jobs, &[(1, 0, Some(true))]);
        assert_eq!(
            classify_nesting(1, jobs.len(), &fake, false),
            Nesting::NestedUnnamed
        );
    }

    #[test]
    fn nesting_requires_candidate_containment_to_prove_immediate_parent() {
        // Job 3's owner is in all three candidates. The apparent deepest
        // candidate is not contained by candidate 1, so it is not proven
        // immediate even though its score is uniquely highest.
        let jobs = owned_jobs(&[0, 1, 2, 3]);
        let fake = FakeMembership::new(
            &jobs,
            &[
                (3, 0, Some(true)),
                (3, 1, Some(true)),
                (3, 2, Some(true)),
                (2, 0, Some(true)),
            ],
        );
        assert_eq!(
            classify_nesting(3, jobs.len(), &fake, true),
            Nesting::NestedUnnamed
        );
    }

    #[test]
    fn nesting_is_unnamed_when_two_siblings_are_equally_deep() {
        // Job 2's owner is in both siblings, and neither contains the other:
        // there is no unique immediate parent, but the nesting is known.
        let jobs = owned_jobs(&[0, 1, 2]);
        let fake = FakeMembership::new(&jobs, &[(2, 0, Some(true)), (2, 1, Some(true))]);
        assert_eq!(
            classify_nesting(2, jobs.len(), &fake, true),
            Nesting::NestedUnnamed
        );
    }

    #[test]
    fn nesting_is_unnamed_when_a_candidate_depth_is_unanswered() {
        // Two candidates, and whether the deeper one contains the other is
        // unknown: naming either would be a guess.
        let jobs = owned_jobs(&[0, 1, 2]);
        let fake = FakeMembership::new(
            &jobs,
            &[(2, 0, Some(true)), (2, 1, Some(true)), (1, 0, None)],
        );
        assert_eq!(
            classify_nesting(2, jobs.len(), &fake, true),
            Nesting::NestedUnnamed
        );
    }
}
