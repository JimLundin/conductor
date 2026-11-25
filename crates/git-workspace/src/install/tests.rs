use super::{Head, Index, Observation, Progress, Step};

const PREPARED: Observation = Observation {
    progress: Progress::Prepared,
    head: Head::Original,
    index: Index::Original,
    unchanged: true,
};

#[test]
fn a_changed_snapshot_records_each_write_before_it_happens() {
    assert_eq!(PREPARED.next(), Step::BeginIndex);
    let started = Observation {
        progress: Progress::IndexStarted,
        ..PREPARED
    };
    assert_eq!(started.next(), Step::WriteIndex);
    let indexed = Observation {
        index: Index::Candidate,
        ..started
    };
    assert_eq!(indexed.next(), Step::BeginBranch);
    let publishing = Observation {
        progress: Progress::BranchStarted,
        ..indexed
    };
    assert_eq!(publishing.next(), Step::WriteBranch);
    assert_eq!(
        Observation {
            head: Head::Published,
            ..publishing
        }
        .next(),
        Step::Receipt
    );
}

#[test]
fn unchanged_snapshot_can_complete_without_writing_branch_or_index() {
    assert_eq!(
        Observation {
            head: Head::Unchanged,
            index: Index::Candidate,
            ..PREPARED
        }
        .next(),
        Step::Receipt
    );
}

#[test]
fn observing_the_candidate_alone_cannot_complete_an_unattributed_publication() {
    assert_eq!(
        Observation {
            head: Head::Changed,
            index: Index::Candidate,
            progress: Progress::BranchStarted,
            ..PREPARED
        }
        .next(),
        Step::Refuse
    );
    assert_eq!(
        Observation {
            head: Head::Published,
            index: Index::Candidate,
            ..PREPARED
        }
        .next(),
        Step::Refuse
    );
    assert_eq!(
        Observation {
            unchanged: false,
            head: Head::Published,
            index: Index::Candidate,
            progress: Progress::BranchStarted
        }
        .next(),
        Step::Refuse
    );
}
