use vcs::{Checkout, DiffExtent, Entry, FilePatch, Merge};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Index {
    Working,
    Scratch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Revision {
    Head,
    Named(String),
    Commit(String),
    Tree(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Instruction {
    Root,
    ProbeRoot,
    CommittedHead,
    GitDirectory,
    CommonDirectory,
    RootCommits,
    Resolve(Revision),
    Subtree {
        commit: String,
        path: String,
    },
    AddWorktree {
        branch: String,
        path: String,
        start: String,
    },
    HeadReference,
    ReadTree {
        revision: String,
        index: Index,
    },
    Stage {
        index: Index,
    },
    WriteTree {
        index: Index,
    },
    Commit {
        tree: String,
        parents: Vec<String>,
        message: String,
    },
    UpdateReference {
        reference: String,
        commit: String,
        expected: Option<String>,
    },
    DeleteReference {
        reference: String,
    },
    DiffNames {
        previous: String,
        commit: String,
    },
    DiffPatch {
        previous: String,
        commit: String,
        path: String,
    },
    RemoveWorktree {
        path: String,
    },
    PruneWorktrees,
    References {
        prefix: String,
        limit: usize,
    },
    Origin {
        push: bool,
    },
    Push {
        branch: String,
    },
    CountAhead {
        base: String,
        tip: String,
    },
    Ancestor {
        base: String,
        tip: String,
    },
    Worktrees,
    Merge {
        base: String,
        tip: String,
    },
    Integrate {
        branch: String,
        commit: String,
        expected: String,
        receipt: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Text(String),
    Count(u64),
    Found(bool),
    Names {
        entries: Vec<Entry>,
        extent: DiffExtent,
    },
    Patch(FilePatch),
    Worktrees(Vec<Checkout>),
    Merge(Merge),
    References(Vec<String>),
}
