use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Candidate {
    pub name: String,
    pub pid: u32,
}

pub(super) fn matches(
    name: &str,
    pid: u32,
    own: u32,
    root: u32,
    ancestors: &HashSet<u32>,
    wanted: &[String],
    attempted: &mut HashSet<u32>,
) -> bool {
    // Only a process whose name was asked for is remembered: a reused PID of
    // another image must not be skipped later.
    if !wanted.iter().any(|n| n.eq_ignore_ascii_case(name)) || !attempted.insert(pid) {
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
    attempted: &mut HashSet<u32>,
) -> Vec<Candidate> {
    rows.into_iter()
        .filter(|r| matches(&r.name, r.pid, own, root, ancestors, wanted, attempted))
        .collect()
}

/// Run one selected candidate through the injectable OS decision seam. A candidate
/// is marked attempted before any potentially failing query, so it is never retried.
pub(super) fn attempt(
    pid: u32,
    attempted: &mut HashSet<u32>,
    same_user: impl FnOnce() -> Option<bool>,
    already_in_job: impl FnOnce() -> Option<bool>,
    assign: impl FnOnce() -> bool,
) -> AttemptResult {
    if !attempted.insert(pid) {
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

    #[test]
    fn selection_filters_exact_names_and_protected_pids_once() {
        let ancestors = HashSet::from([12, 13]);
        let wanted = vec!["match.exe".to_string()];
        let rows = vec![
            Candidate {
                name: "MATCH.EXE".into(),
                pid: 10,
            },
            Candidate {
                name: "match.exe".into(),
                pid: 10,
            },
            Candidate {
                name: "other.exe".into(),
                pid: 11,
            },
            Candidate {
                name: "match.exe".into(),
                pid: 12,
            },
            Candidate {
                name: "match.exe".into(),
                pid: 13,
            },
            Candidate {
                name: "match.exe".into(),
                pid: 14,
            },
            Candidate {
                name: "match.exe".into(),
                pid: 1,
            },
            Candidate {
                name: "match.exe".into(),
                pid: 15,
            },
            Candidate {
                name: "match.exe".into(),
                pid: 0,
            },
            Candidate {
                name: "match.exe".into(),
                pid: 4,
            },
        ];
        let mut attempted = HashSet::new();
        let selected = select(rows.clone(), 1, 14, &ancestors, &wanted, &mut attempted);
        assert_eq!(selected.iter().map(|r| r.pid).collect::<Vec<_>>(), [10, 15]);
        // pid 11 (another image) is not remembered.
        assert_eq!(attempted, HashSet::from([10, 12, 13, 14, 1, 15, 0, 4]));
        assert!(select(rows, 1, 14, &ancestors, &wanted, &mut attempted).is_empty());
    }

    #[test]
    fn exact_case_insensitive_name_selects_only_valid_pid() {
        let wanted = vec!["match.exe".to_string()];
        let mut attempted = HashSet::new();
        let rows = [
            Candidate {
                name: "MATCH.EXE".into(),
                pid: 20,
            },
            Candidate {
                name: "match.exe.extra".into(),
                pid: 21,
            },
        ];
        assert_eq!(
            select(rows, 1, 2, &HashSet::new(), &wanted, &mut attempted)
                .iter()
                .map(|r| r.pid)
                .collect::<Vec<_>>(),
            [20]
        );
    }

    #[test]
    fn fake_decision_seam_covers_query_and_assignment_outcomes_once() {
        let mut attempted = HashSet::new();
        assert_eq!(
            attempt(
                1,
                &mut attempted,
                || Some(false),
                || panic!("queried"),
                || panic!("assigned")
            ),
            AttemptResult::OtherUser
        );
        assert_eq!(
            attempt(
                2,
                &mut attempted,
                || Some(true),
                || Some(true),
                || panic!("assigned")
            ),
            AttemptResult::AlreadyInJob
        );
        assert_eq!(
            attempt(
                3,
                &mut attempted,
                || None,
                || panic!("queried"),
                || panic!("assigned")
            ),
            AttemptResult::QueryFailed
        );
        assert_eq!(
            attempt(
                3,
                &mut attempted,
                || panic!("retried"),
                || Some(false),
                || true
            ),
            AttemptResult::AlreadyAttempted
        );
        assert_eq!(
            attempt(
                4,
                &mut attempted,
                || Some(true),
                || None,
                || panic!("assigned")
            ),
            AttemptResult::QueryFailed
        );
        assert_eq!(
            attempt(
                4,
                &mut attempted,
                || panic!("retried"),
                || Some(false),
                || true
            ),
            AttemptResult::AlreadyAttempted
        );
        assert_eq!(
            attempt(5, &mut attempted, || Some(true), || Some(false), || true),
            AttemptResult::Assigned
        );
        assert_eq!(
            attempt(6, &mut attempted, || Some(true), || Some(false), || false),
            AttemptResult::AssignmentFailed
        );
        assert_eq!(attempted, HashSet::from([1, 2, 3, 4, 5, 6]));
    }
}
