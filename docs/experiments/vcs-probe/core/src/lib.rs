#![no_std]

pub mod installation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Receipt {
    Missing,
    Matches,
    Wrong,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Branch {
    Expected,
    CandidateOrDescendant,
    Diverged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recovery {
    Publish,
    RepairReceipt,
    Completed,
    Refuse,
}

#[must_use]
pub const fn recover(
    receipt: Receipt,
    branch: Branch,
    history_unchanged: bool,
) -> Recovery {
    match (receipt, branch, history_unchanged) {
        (Receipt::Matches, _, _) => Recovery::Completed,
        (Receipt::Missing, Branch::CandidateOrDescendant, _) => {
            Recovery::RepairReceipt
        }
        (Receipt::Missing, Branch::Expected, true) => Recovery::Publish,
        _ => Recovery::Refuse,
    }
}

#[must_use]
pub const fn preserves_staging(
    staged_matches_head: bool,
    staged_matches_snapshot: bool,
) -> bool {
    staged_matches_head || staged_matches_snapshot
}

#[must_use]
pub const fn classify_receipt(matches_candidate: Option<bool>) -> Receipt {
    match matches_candidate {
        None => Receipt::Missing,
        Some(true) => Receipt::Matches,
        Some(false) => Receipt::Wrong,
    }
}

#[must_use]
pub const fn classify_branch(
    at_expected: bool,
    at_candidate: bool,
    contains_candidate: bool,
) -> Branch {
    match (at_expected, at_candidate || contains_candidate) {
        (_, true) => Branch::CandidateOrDescendant,
        (true, false) => Branch::Expected,
        (false, false) => Branch::Diverged,
    }
}

#[must_use]
pub fn any_checkout(main: bool, linked: &[bool]) -> bool {
    main || linked.contains(&true)
}

#[must_use]
pub const fn guard_retention(action: Recovery, retained: bool) -> Recovery {
    match (action, retained) {
        (Recovery::Completed, _) | (_, true) => action,
        _ => Recovery::Refuse,
    }
}

#[must_use]
pub const fn checkout_branch(action: Recovery, branch: &str) -> Option<&str> {
    match action {
        Recovery::Publish => Some(branch),
        _ => None,
    }
}

#[must_use]
pub const fn recovery_target<'a>(
    action: Recovery,
    branch: &'a str,
    receipt: &'a str,
) -> Option<&'a str> {
    match action {
        Recovery::Publish => Some(branch),
        Recovery::RepairReceipt => Some(receipt),
        _ => None,
    }
}

#[must_use]
pub const fn guard_checkout(action: Recovery, checked_out: bool) -> Recovery {
    match (action, checked_out) {
        (Recovery::Publish, true) => Recovery::Refuse,
        _ => action,
    }
}

#[cfg(test)]
mod tests {
    use super::{Branch, Receipt, Recovery, recover};

    #[test]
    fn unpublished_intent_can_resume_without_a_concurrent_write() {
        assert_eq!(
            recover(Receipt::Missing, Branch::Expected, true),
            Recovery::Publish
        );
    }

    #[test]
    fn completed_branch_update_repairs_its_receipt() {
        assert_eq!(
            recover(Receipt::Missing, Branch::CandidateOrDescendant, false),
            Recovery::RepairReceipt
        );
    }

    #[test]
    fn receipt_prevents_reapplying_a_completed_attempt_after_branch_rewrite() {
        assert_eq!(
            recover(Receipt::Matches, Branch::Diverged, false),
            Recovery::Completed
        );
    }

    #[test]
    fn uncertain_or_inconsistent_progress_requires_reconciliation() {
        assert_eq!(
            recover(Receipt::Missing, Branch::Expected, false),
            Recovery::Refuse
        );
        assert_eq!(
            recover(Receipt::Missing, Branch::Diverged, false),
            Recovery::Refuse
        );
        assert_eq!(
            recover(Receipt::Wrong, Branch::CandidateOrDescendant, false),
            Recovery::Refuse
        );
    }
}
