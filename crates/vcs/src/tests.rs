use alloc::{string::String, vec, vec::Vec};

use crate::discovery::{Probe, observed};
use crate::plan::{
    Integration, Ownership, Snapshot, Trees, index_preserved, integration,
    snapshot,
};

#[test]
fn only_the_top_of_a_repository_with_a_commit_is_observed() {
    let path = "/home/me/conductor";
    let roots = vec![String::from("a"), String::from("b")];
    assert_eq!(
        observed(
            String::from(path),
            Some(path),
            roots.clone(),
            Some(String::from("url"))
        ),
        Some(Probe {
            path: String::from(path),
            roots: roots.clone(),
            remote: Some(String::from("url"))
        })
    );
    assert_eq!(
        observed(String::from(path), Some("/home/me"), roots.clone(), None),
        None
    );
    assert_eq!(observed(String::from(path), None, roots, None), None);
    assert_eq!(
        observed(String::from(path), Some(path), Vec::new(), None),
        None
    );
}

#[test]
fn snapshots_reuse_the_tip_only_when_the_files_match_and_preserve_staging() {
    assert_eq!(snapshot("tree", "tree"), Snapshot::Tip);
    assert_eq!(snapshot("tree", "changed"), Snapshot::Commit);
    assert!(index_preserved("branch", "files", "branch"));
    assert!(index_preserved("branch", "files", "files"));
    assert!(!index_preserved("branch", "files", "staged-only"));
}

#[test]
fn removing_a_workspace_preserves_only_changes_absent_from_its_branch() {
    let trees = Trees {
        branch: String::from("branch"),
        worktree: Some(String::from("changed")),
    };
    assert_eq!(trees.changed(), Some("changed"));
    assert_eq!(
        Trees {
            worktree: Some(String::from("branch")),
            ..trees.clone()
        }
        .changed(),
        None
    );
    assert_eq!(
        Trees {
            worktree: None,
            ..trees
        }
        .changed(),
        None
    );
}

#[test]
fn integration_refuses_checked_out_targets_and_preserves_no_change_results() {
    assert_eq!(
        integration(0, Some("/checkout"), false),
        Integration::Unchanged
    );
    assert_eq!(
        integration(2, Some("/checkout"), true),
        Integration::CheckedOut(String::from("/checkout"))
    );
    assert_eq!(integration(2, None, true), Integration::FastForward);
    assert_eq!(integration(2, None, false), Integration::Merge);
}

#[test]
fn ownership_requires_every_recorded_workspace_field() {
    let ownership = Ownership {
        repository: String::from("/repo"),
        original: String::from("/repo/sub"),
        worktree: String::from("/worktrees/work"),
        directory: String::from("/worktrees/work/sub"),
        branch: String::from("work"),
    };
    let recorded =
        "/repo\n/repo/sub\n/worktrees/work\n/worktrees/work/sub\nwork\n";
    assert!(ownership.matches(recorded));
    assert!(
        !Ownership {
            branch: String::from("another"),
            ..ownership.clone()
        }
        .matches(recorded)
    );
    assert!(
        !Ownership {
            directory: String::from("/elsewhere"),
            ..ownership
        }
        .matches(recorded)
    );
}

#[test]
fn repository_identity_preserves_local_and_network_path_case() {
    assert_eq!(
        crate::remote::normal("https://GitHub.com/JimLundin/conductor.git"),
        crate::remote::normal("git@github.com:JimLundin/conductor")
    );
    assert_ne!(
        crate::remote::normal("https://github.com/JimLundin/conductor.git"),
        crate::remote::normal("https://github.com/jimlundin/conductor.git")
    );
    assert_ne!(
        crate::remote::normal("/srv/a@Home.git"),
        crate::remote::normal("/srv/a@home.git")
    );
    assert_ne!(
        crate::remote::normal("/srv/a@Home.git"),
        crate::remote::normal("/srv/Home.git")
    );
    assert_eq!(crate::remote::display("/srv/a@Home.git"), "/srv/a@Home.git");
}

#[test]
fn workspace_ownership_refuses_a_different_repository_or_checked_out_branch() {
    let owned = Ownership {
        repository: String::from("/repo"),
        original: String::from("/repo/sub"),
        worktree: String::from("/worktrees/work"),
        directory: String::from("/worktrees/work/sub"),
        branch: String::from("work"),
    };
    let mut observed = crate::plan::RepositoryFacts {
        root: String::from("/worktrees/work"),
        branch: String::from("work"),
        common: String::from("/repo/.git"),
        source_common: String::from("/repo/.git"),
        linked: true,
    };
    assert_eq!(owned.repository(&observed), None);
    observed.common = String::from("/other/.git");
    assert_eq!(
        owned.repository(&observed),
        Some(crate::Error::Failed(String::from(
            "common repository: metadata differs"
        )))
    );
    observed.common = String::from("/repo/.git");
    observed.branch = String::from("another");
    assert_eq!(
        owned.repository(&observed),
        Some(crate::Error::Failed(String::from(
            "owned branch: metadata differs"
        )))
    );
}
