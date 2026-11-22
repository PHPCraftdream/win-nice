//! The single definition of "the inventory is complete enough for a verdict".

use super::{OwnerFailure, Uninspected};

/// Accumulates every query whose failure could make a chain verdict wrong.
/// `complete` is the ONE place that conjunction is defined: a complete
/// inventory means the process snapshot worked, the handle table and job type
/// were resolved, every same-user owner was inspected, and every job that was
/// found was duplicated and fully queried.
#[derive(Debug, Default)]
pub(crate) struct Inventory {
    failures: Vec<OwnerFailure>,
    incomplete: bool,
}

impl Inventory {
    /// The completeness flag and every failure recorded, in the order of the
    /// queries that produced them.
    pub(crate) fn into_parts(self) -> (bool, Vec<OwnerFailure>) {
        let complete = self.complete();
        // A handful of rows at most: copying them keeps one reader per field.
        let failures = self.failures().to_vec();
        (complete, failures)
    }

    /// True when no query that a verdict depends on failed.
    pub(crate) fn complete(&self) -> bool {
        !self.incomplete
    }

    /// Every failure recorded so far.
    pub(crate) fn failures(&self) -> &[OwnerFailure] {
        &self.failures
    }

    /// Discovery that precedes every per-owner step failed: no owner could be
    /// inspected, whether or not any owner was even recognised.
    pub(crate) fn discovery_failed(&mut self, owners: &[(u32, String)], reason: Uninspected) {
        self.incomplete = true;
        for (pid, image) in owners {
            self.record(*pid, image, reason);
        }
    }

    /// One owner could not be inspected. A foreign owner (`DifferentUser`) is
    /// not part of the current user's inventory, so it leaves it complete.
    pub(crate) fn owner_failed(&mut self, pid: u32, image: &str, reason: Uninspected) {
        if reason != Uninspected::DifferentUser {
            self.incomplete = true;
        }
        self.record(pid, image, reason);
    }

    /// A job object that no available handle could turn into a row: counted
    /// against its first owner, `Uninspected::JobQuery`.
    pub(crate) fn job_failed(&mut self, pid: u32, image: &str) {
        self.incomplete = true;
        self.record(pid, image, Uninspected::JobQuery);
    }

    /// A required query failed where the report still shows the row (limit or
    /// accounting values unknown, or the process snapshot failed).
    pub(crate) fn mark_incomplete(&mut self) {
        self.incomplete = true;
    }

    fn record(&mut self, pid: u32, image: &str, reason: Uninspected) {
        self.failures.push(OwnerFailure {
            pid,
            image: image.to_string(),
            reason,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(pid: u32) -> (u32, String) {
        (pid, "capc.exe".to_string())
    }

    fn failure(pid: u32, image: &str, reason: Uninspected) -> OwnerFailure {
        OwnerFailure {
            pid,
            image: image.to_string(),
            reason,
        }
    }

    #[test]
    fn a_fresh_inventory_is_complete() {
        let inventory = Inventory::default();
        assert!(inventory.complete());
        assert!(inventory.failures().is_empty());
    }

    #[test]
    fn discovery_failure_is_incomplete_even_without_owners() {
        let mut inventory = Inventory::default();
        inventory.discovery_failed(&[], Uninspected::HandleTable);
        assert!(!inventory.complete());
        assert!(inventory.failures().is_empty());
    }

    #[test]
    fn discovery_failure_names_every_owner_in_order() {
        let mut inventory = Inventory::default();
        inventory.discovery_failed(&[owner(300), owner(100)], Uninspected::JobType);
        assert!(!inventory.complete());
        assert_eq!(
            inventory.failures(),
            &[
                failure(300, "capc.exe", Uninspected::JobType),
                failure(100, "capc.exe", Uninspected::JobType),
            ]
        );
    }

    #[test]
    fn foreign_owners_alone_leave_the_inventory_complete() {
        let mut inventory = Inventory::default();
        inventory.owner_failed(10, "high.exe", Uninspected::DifferentUser);
        inventory.owner_failed(20, "idle.exe", Uninspected::DifferentUser);
        assert!(inventory.complete());
        assert_eq!(inventory.failures().len(), 2);
    }

    #[test]
    fn a_same_user_failure_clears_the_completeness() {
        for reason in [
            Uninspected::AccessDenied,
            Uninspected::TokenQuery,
            Uninspected::JobQuery,
        ] {
            let mut inventory = Inventory::default();
            inventory.owner_failed(10, "capc.exe", reason);
            assert!(!inventory.complete(), "{reason:?}");
        }
    }

    #[test]
    fn job_failure_clears_it_and_record_one_failure() {
        let mut inventory = Inventory::default();
        inventory.job_failed(4242, "wnice.exe");
        assert!(!inventory.complete());
        assert_eq!(
            inventory.failures(),
            &[failure(4242, "wnice.exe", Uninspected::JobQuery)]
        );
    }

    #[test]
    fn mark_incomplete_clears_it_without_a_failure() {
        let mut inventory = Inventory::default();
        inventory.mark_incomplete();
        assert!(!inventory.complete());
        assert!(inventory.failures().is_empty());
    }

    #[test]
    fn into_parts_returns_the_flag_and_the_failures() {
        let mut inventory = Inventory::default();
        inventory.owner_failed(7, "capc.exe", Uninspected::DifferentUser);
        inventory.job_failed(8, "wnice.exe");
        let (complete, failures) = inventory.into_parts();
        assert!(!complete);
        assert_eq!(
            failures,
            vec![
                failure(7, "capc.exe", Uninspected::DifferentUser),
                failure(8, "wnice.exe", Uninspected::JobQuery),
            ]
        );
    }
}
