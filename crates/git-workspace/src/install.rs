#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    Prepared,
    IndexStarted,
    BranchStarted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Head {
    Original,
    Unchanged,
    Published,
    Changed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Index {
    Original,
    Candidate,
    Changed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Observation {
    pub progress: Progress,
    pub head: Head,
    pub index: Index,
    pub unchanged: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    BeginIndex,
    WriteIndex,
    BeginBranch,
    WriteBranch,
    Receipt,
    Refuse,
}

impl Observation {
    #[must_use]
    pub const fn next(self) -> Step {
        if !self.unchanged {
            return Step::Refuse;
        }
        match (self.head, self.index, self.progress) {
            (Head::Unchanged, Index::Candidate, Progress::Prepared)
            | (Head::Published, Index::Candidate, Progress::BranchStarted) => {
                Step::Receipt
            }
            (
                Head::Original,
                Index::Original | Index::Candidate,
                Progress::Prepared,
            ) => Step::BeginIndex,
            (Head::Original, Index::Original, Progress::IndexStarted) => {
                Step::WriteIndex
            }
            (Head::Original, Index::Candidate, Progress::IndexStarted) => {
                Step::BeginBranch
            }
            (Head::Original, Index::Candidate, Progress::BranchStarted) => {
                Step::WriteBranch
            }
            _ => Step::Refuse,
        }
    }
}

#[cfg(test)]
#[path = "install/tests.rs"]
mod tests;
