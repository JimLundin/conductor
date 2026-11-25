use crate::Receipt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Index {
    Original,
    Candidate,
    Changed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Head {
    Original,
    Candidate,
    Changed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preconditions {
    pub owned: bool,
    pub retained: bool,
    pub files_unchanged: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    Prepared,
    IndexStarted,
    BranchStarted,
    Invalid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Observation {
    pub receipt: Receipt,
    pub preconditions: Preconditions,
    pub index: Index,
    pub head: Head,
    pub progress: Progress,
    pub history_unchanged: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    BeginIndex,
    InstallIndex,
    BeginBranch,
    PublishBranch,
    RecordReceipt,
    Completed,
    Refuse,
}

#[must_use]
pub const fn next(observed: Observation) -> Step {
    if matches!(observed.receipt, Receipt::Matches) {
        return Step::Completed;
    }
    if !matches!(observed.receipt, Receipt::Missing)
        || !observed.preconditions.owned
        || !observed.preconditions.retained
        || !observed.preconditions.files_unchanged
    {
        return Step::Refuse;
    }
    match observed.progress {
        Progress::Prepared => before_index(observed),
        Progress::IndexStarted => after_index_started(observed),
        Progress::BranchStarted => after_branch_started(observed),
        Progress::Invalid => Step::Refuse,
    }
}

#[must_use]
pub const fn progress(index_started: bool, branch_started: bool) -> Progress {
    match (index_started, branch_started) {
        (false, false) => Progress::Prepared,
        (true, false) => Progress::IndexStarted,
        (true, true) => Progress::BranchStarted,
        (false, true) => Progress::Invalid,
    }
}

const fn before_index(observed: Observation) -> Step {
    match (observed.head, observed.index, observed.history_unchanged) {
        (Head::Original, Index::Original | Index::Candidate, true) => {
            Step::BeginIndex
        }
        _ => Step::Refuse,
    }
}

const fn after_index_started(observed: Observation) -> Step {
    match (observed.head, observed.index, observed.history_unchanged) {
        (Head::Original, Index::Original, true) => Step::InstallIndex,
        (Head::Original, Index::Candidate, true) => Step::BeginBranch,
        _ => Step::Refuse,
    }
}

const fn after_branch_started(observed: Observation) -> Step {
    match (observed.head, observed.index, observed.history_unchanged) {
        (Head::Original, Index::Candidate, true) => Step::PublishBranch,
        (Head::Candidate, Index::Candidate, _) => Step::RecordReceipt,
        _ => Step::Refuse,
    }
}

#[must_use]
pub const fn index(original: bool, candidate: bool) -> Index {
    match (original, candidate) {
        (_, true) => Index::Candidate,
        (true, false) => Index::Original,
        _ => Index::Changed,
    }
}

#[must_use]
pub const fn head(original: bool, candidate: bool) -> Head {
    match (original, candidate) {
        (_, true) => Head::Candidate,
        (true, false) => Head::Original,
        _ => Head::Changed,
    }
}

#[must_use]
pub const fn publication<'a>(
    step: Step,
    branch: &'a str,
    receipt: &'a str,
) -> Option<&'a str> {
    match step {
        Step::PublishBranch => Some(branch),
        Step::RecordReceipt => Some(receipt),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cancellation {
    BeforeDispatch,
    Running,
    ReturnedApplied,
    ReturnedUncertain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    NotStarted,
    InFlight,
    Applied,
    Unknown,
}

#[must_use]
pub const fn cancelled(at: Cancellation) -> Effect {
    match at {
        Cancellation::BeforeDispatch => Effect::NotStarted,
        Cancellation::Running => Effect::InFlight,
        Cancellation::ReturnedApplied => Effect::Applied,
        Cancellation::ReturnedUncertain => Effect::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Head, Index, Observation, Preconditions, Progress, Step, next,
    };
    use crate::Receipt;

    const PREPARED: Observation = Observation {
        receipt: Receipt::Missing,
        preconditions: Preconditions {
            owned: true,
            retained: true,
            files_unchanged: true,
        },
        index: Index::Original,
        head: Head::Original,
        progress: Progress::Prepared,
        history_unchanged: true,
    };

    #[test]
    fn partial_staging_and_unattributed_effects_refuse() {
        assert_eq!(
            next(Observation {
                index: Index::Changed,
                ..PREPARED
            }),
            Step::Refuse
        );
        assert_eq!(
            next(Observation {
                head: Head::Candidate,
                ..PREPARED
            }),
            Step::Refuse
        );
        assert_eq!(
            next(Observation {
                progress: Progress::Invalid,
                ..PREPARED
            }),
            Step::Refuse
        );
    }

    #[test]
    fn uncertain_index_and_branch_rewrites_refuse() {
        assert_eq!(
            next(Observation {
                progress: Progress::IndexStarted,
                history_unchanged: false,
                ..PREPARED
            }),
            Step::Refuse
        );
        assert_eq!(
            next(Observation {
                progress: Progress::BranchStarted,
                ..PREPARED
            }),
            Step::Refuse
        );
        assert_eq!(
            next(Observation {
                preconditions: Preconditions {
                    retained: false,
                    ..PREPARED.preconditions
                },
                ..PREPARED
            }),
            Step::Refuse
        );
    }

    #[test]
    fn complete_receipt_never_repairs_later_user_edits() {
        assert_eq!(
            next(Observation {
                receipt: Receipt::Matches,
                preconditions: Preconditions {
                    owned: false,
                    retained: false,
                    files_unchanged: false
                },
                index: Index::Changed,
                head: Head::Changed,
                ..PREPARED
            }),
            Step::Completed
        );
    }
}
