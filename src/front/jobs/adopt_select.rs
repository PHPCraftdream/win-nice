use std::collections::HashSet;

/// One process of the snapshot. `created` (FILETIME ticks, 0 when it could not
/// be read) with the PID names the process: a PID the system hands to a new
/// process is a new candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Candidate {
    pub name: String,
    pub pid: u32,
    pub created: u64,
}

/// Process identity: PID and creation time.
pub(super) type Identity = (u32, u64);

/// Whether the image `name` was asked for.
pub(super) fn is_wanted(name: &str, wanted: &[String]) -> bool {
    wanted.iter().any(|n| n.eq_ignore_ascii_case(name))
}

pub(super) fn matches(
    candidate: &Candidate,
    own: u32,
    root: u32,
    ancestors: &HashSet<u32>,
    wanted: &[String],
    attempted: &mut HashSet<Identity>,
) -> bool {
    let pid = candidate.pid;
    // Only a process whose name was asked for is remembered: a reused PID of
    // another image must not be skipped later.
    if !is_wanted(&candidate.name, wanted) || !attempted.insert((pid, candidate.created)) {
        return false;
    }
    pid != 0 && pid != 4 && pid != own && pid != root && !ancestors.contains(&pid)
}

pub(super) fn select(
    rows: impl IntoIterator<Item = Candidate>,
    own: u32,
    root: u32,
    ancestors: &HashSet<u32>,
    wanted: &[String],
    attempted: &mut HashSet<Identity>,
) -> Vec<Candidate> {
    rows.into_iter()
        .filter(|r| matches(r, own, root, ancestors, wanted, attempted))
        .collect()
}

/// Run one selected candidate through the injectable OS decision seam. A candidate
/// is marked attempted before any potentially failing query, so it is never retried.
pub(super) fn attempt(
    id: Identity,
    attempted: &mut HashSet<Identity>,
    same_user: impl FnOnce() -> Option<bool>,
    already_in_job: impl FnOnce() -> Option<bool>,
    assign: impl FnOnce() -> bool,
) -> AttemptResult {
    if !attempted.insert(id) {
        return AttemptResult::AlreadyAttempted;
    }
    match same_user() {
        Some(true) => {}
        Some(false) => return AttemptResult::OtherUser,
        None => return AttemptResult::QueryFailed,
    }
    match already_in_job() {
        Some(false) => {}
        Some(true) => return AttemptResult::AlreadyInJob,
        None => return AttemptResult::QueryFailed,
    }
    if assign() {
        AttemptResult::Assigned
    } else {
        AttemptResult::AssignmentFailed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AttemptResult {
    AlreadyAttempted,
    OtherUser,
    AlreadyInJob,
    QueryFailed,
    Assigned,
    AssignmentFailed,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(name: &str, pid: u32, created: u64) -> Candidate {
        Candidate {
            name: name.into(),
            pid,
            created,
        }
    }

    #[test]
    fn selection_filters_exact_names_and_protected_pids_once() {
        let ancestors = HashSet::from([12, 13]);
        let wanted = vec!["match.exe".to_string()];
        let rows = vec![
            cand("MATCH.EXE", 10, 1),
            cand("match.exe", 10, 1),
            cand("other.exe", 11, 1),
            cand("match.exe", 12, 1),
            cand("match.exe", 13, 1),
            cand("match.exe", 14, 1),
            cand("match.exe", 1, 1),
            cand("match.exe", 15, 1),
            cand("match.exe", 0, 1),
            cand("match.exe", 4, 1),
        ];
        let mut attempted = HashSet::new();
        let selected = select(rows.clone(), 1, 14, &ancestors, &wanted, &mut attempted);
        assert_eq!(selected.iter().map(|r| r.pid).collect::<Vec<_>>(), [10, 15]);
        // pid 11 (another image) is not remembered.
        assert_eq!(
            attempted,
            HashSet::from([
                (10, 1),
                (12, 1),
                (13, 1),
                (14, 1),
                (1, 1),
                (15, 1),
                (0, 1),
                (4, 1)
            ])
        );
        assert!(select(rows, 1, 14, &ancestors, &wanted, &mut attempted).is_empty());
    }

    #[test]
    fn exact_case_insensitive_name_selects_only_valid_pid() {
        let wanted = vec!["match.exe".to_string()];
        let mut attempted = HashSet::new();
        let rows = [cand("MATCH.EXE", 20, 1), cand("match.exe.extra", 21, 1)];
        assert_eq!(
            select(rows, 1, 2, &HashSet::new(), &wanted, &mut attempted)
                .iter()
                .map(|r| r.pid)
                .collect::<Vec<_>>(),
            [20]
        );
    }

    #[test]
    fn a_reused_pid_is_a_new_candidate() {
        let wanted = vec!["node.exe".to_string()];
        let mut attempted = HashSet::new();
        let pick = |created: u64, attempted: &mut HashSet<Identity>| {
            select(
                [cand("node.exe", 40, created)],
                1,
                2,
                &HashSet::new(),
                &wanted,
                attempted,
            )
            .len()
        };
        assert_eq!(pick(100, &mut attempted), 1);
        assert_eq!(pick(100, &mut attempted), 0, "same process: once");
        // The server restarted and the system gave its PID to the new process.
        assert_eq!(pick(200, &mut attempted), 1);
        assert_eq!(pick(200, &mut attempted), 0);
        assert_eq!(attempted, HashSet::from([(40, 100), (40, 200)]));
    }

    #[test]
    fn protected_pids_stay_protected_whatever_their_creation_time() {
        let wanted = vec!["node.exe".to_string()];
        let ancestors = HashSet::from([30]);
        let mut attempted = HashSet::new();
        for created in [0, 1, 2] {
            let rows = [
                cand("node.exe", 30, created),
                cand("node.exe", 31, created), // own
                cand("node.exe", 32, created), // the command
                cand("node.exe", 4, created),
            ];
            assert!(select(rows, 31, 32, &ancestors, &wanted, &mut attempted).is_empty());
        }
    }

    #[test]
    fn only_asked_for_names_are_wanted() {
        let wanted = vec!["a.exe".to_string(), "b.exe".to_string()];
        assert!(is_wanted("A.EXE", &wanted));
        assert!(is_wanted("b.exe", &wanted));
        assert!(!is_wanted("c.exe", &wanted));
        assert!(!is_wanted("a.exe.extra", &wanted));
    }

    #[test]
    fn fake_decision_seam_covers_query_and_assignment_outcomes_once() {
        let mut attempted = HashSet::new();
        assert_eq!(
            attempt(
                (1, 7),
                &mut attempted,
                || Some(false),
                || panic!("queried"),
                || panic!("assigned")
            ),
            AttemptResult::OtherUser
        );
        assert_eq!(
            attempt(
                (2, 7),
                &mut attempted,
                || Some(true),
                || Some(true),
                || panic!("assigned")
            ),
            AttemptResult::AlreadyInJob
        );
        assert_eq!(
            attempt(
                (3, 7),
                &mut attempted,
                || None,
                || panic!("queried"),
                || panic!("assigned")
            ),
            AttemptResult::QueryFailed
        );
        assert_eq!(
            attempt(
                (3, 7),
                &mut attempted,
                || panic!("retried"),
                || Some(false),
                || true
            ),
            AttemptResult::AlreadyAttempted
        );
        assert_eq!(
            attempt(
                (4, 7),
                &mut attempted,
                || Some(true),
                || None,
                || panic!("assigned")
            ),
            AttemptResult::QueryFailed
        );
        assert_eq!(
            attempt(
                (4, 7),
                &mut attempted,
                || panic!("retried"),
                || Some(false),
                || true
            ),
            AttemptResult::AlreadyAttempted
        );
        assert_eq!(
            attempt(
                (5, 7),
                &mut attempted,
                || Some(true),
                || Some(false),
                || true
            ),
            AttemptResult::Assigned
        );
        assert_eq!(
            attempt(
                (6, 7),
                &mut attempted,
                || Some(true),
                || Some(false),
                || false
            ),
            AttemptResult::AssignmentFailed
        );
        // The same PID with another creation time is another process.
        assert_eq!(
            attempt(
                (5, 8),
                &mut attempted,
                || Some(true),
                || Some(false),
                || true
            ),
            AttemptResult::Assigned
        );
        assert_eq!(
            attempted,
            HashSet::from([(1, 7), (2, 7), (3, 7), (4, 7), (5, 7), (6, 7), (5, 8)])
        );
    }
}
