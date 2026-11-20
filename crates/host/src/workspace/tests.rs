use core::ops::ControlFlow;

use proto::{
    Checkpoint, CheckpointId, DiffExtent, FilePatch, HistoryAnchor, HostPath,
    Kept, SessionId, UndoAvailability, Workspace,
};

use super::{Answer, GitError, Output, Plan, Step, checkpoints, head};

const SESSION: SessionId = SessionId(1);
const WORKTREE: &str = "/data/worktrees/00000000000000000000000001";
const BRANCH: &str = "conductor/00000000000000000000000001";
const HEAD: &str = "refs/heads/conductor/00000000000000000000000001";
const DIRECTORY: &str = "/repo/.git/worktrees/00000000000000000000000001";
const CHECKPOINT: &str = "refs/conductor/00000000000000000000000001/7";
const PREFIX: &str = "refs/conductor/00000000000000000000000001/";

fn workspace() -> Workspace {
    let path = |text: &str| HostPath(String::from(text));
    Workspace {
        repository: path("/repo"),
        original: path("/repo/sub"),
        worktree: path(WORKTREE),
        directory: path(&format!("{WORKTREE}/sub")),
        branch: String::from(BRANCH),
    }
}

fn output(bytes: impl AsRef<[u8]>) -> Answer {
    Answer::Git(Output {
        bytes: bytes.as_ref().to_vec(),
        success: true,
        error: String::new(),
    })
}

fn failed(error: &str) -> Answer {
    Answer::Git(Output {
        bytes: Vec::new(),
        success: false,
        error: String::from(error),
    })
}

fn path(text: &str) -> Answer {
    Answer::Path(Some(String::from(text)))
}

fn run<T>(
    mut plan: Plan<'_, T>,
    mut perform: impl FnMut(Step) -> Answer,
) -> Result<T, GitError> {
    let mut answer = None;
    loop {
        match plan.advance(answer) {
            ControlFlow::Continue((step, remaining)) => {
                plan = remaining;
                answer = Some(Ok(perform(step)));
            }
            ControlFlow::Break(result) => return result,
        }
    }
}

fn ownership() -> String {
    format!("/repo\n/repo/sub\n{WORKTREE}\n{WORKTREE}/sub\n{BRANCH}\n")
}

fn valid(step: Step) -> Answer {
    match step {
        Step::Canonicalize(ref value) => path(value),
        Step::Relative {
            path: original,
            root,
            ..
        } => {
            assert_eq!(
                (original.as_str(), root.as_str()),
                ("/repo/sub", "/repo")
            );
            path("sub")
        }
        Step::Join { root, suffix } => path(&format!("{root}/{suffix}")),
        Step::Parent(root) => {
            assert_eq!(root, WORKTREE);
            path("/data/worktrees")
        }
        Step::Within { path, root } => {
            assert_eq!(
                (path.as_str(), root.as_str()),
                (DIRECTORY, "/repo/.git/worktrees")
            );
            Answer::Found(true)
        }
        Step::ReadOwner(directory) => {
            assert_eq!(directory, DIRECTORY);
            Answer::Text(ownership())
        }
        Step::CloseIndex => Answer::Done,
        Step::OpenIndex(directory) => {
            assert_eq!(directory, DIRECTORY);
            Answer::Done
        }
        Step::Git {
            path,
            args,
            index: false,
            identity: false,
        } => {
            let words = args.iter().map(String::as_str).collect::<Vec<_>>();
            match words.as_slice() {
                ["rev-parse", "--show-toplevel"] if path == WORKTREE => {
                    output(WORKTREE)
                }
                ["symbolic-ref", "HEAD"] => output(HEAD),
                ["rev-parse", "--path-format=absolute", "--git-common-dir"] => {
                    output("/repo/.git")
                }
                ["rev-parse", "--absolute-git-dir"] => output(DIRECTORY),
                _ => failed(&format!("unexpected Git step {args:?} in {path}")),
            }
        }
        other => failed(&format!("unexpected step {other:?}")),
    }
}

#[test]
fn create_probes_the_selected_base_and_writes_ownership_only_for_a_new_worktree()
 {
    let workspace = workspace();
    assert_eq!(head(&workspace), HEAD);
    assert_eq!(checkpoints(&workspace).as_deref(), Ok(PREFIX));
    for exists in [false, true] {
        let mut created = Vec::new();
        let result = run(
            Plan::create(&workspace.original, SESSION, Some("issue")),
            |step| match step {
                Step::WorktreeParent => path("/data/worktrees"),
                Step::Exists(ref value) if value == WORKTREE => {
                    Answer::Found(exists)
                }
                Step::WriteOwner { directory, marker } => {
                    assert_eq!(directory, DIRECTORY);
                    assert_eq!(marker, ownership());
                    created.push("marker");
                    Answer::Done
                }
                Step::Git {
                    path,
                    ref args,
                    index: false,
                    identity: false,
                } if path != WORKTREE => {
                    let words =
                        args.iter().map(String::as_str).collect::<Vec<_>>();
                    match words.as_slice() {
                        ["rev-parse", "--show-toplevel"]
                            if path == "/repo/sub" =>
                        {
                            output("/repo\n")
                        }
                        ["rev-parse", "--verify", "issue^{commit}"] => {
                            output("start\n")
                        }
                        ["cat-file", "-t", "start:sub"] => output("tree\n"),
                        [
                            "worktree",
                            "add",
                            "-b",
                            BRANCH,
                            WORKTREE,
                            "start",
                        ] => {
                            created.push("worktree");
                            output("")
                        }
                        _ => valid(Step::Git {
                            path,
                            args: args.clone(),
                            index: false,
                            identity: false,
                        }),
                    }
                }
                other => valid(other),
            },
        );
        assert_eq!(result, Ok(workspace.clone()));
        assert_eq!(
            created.as_slice(),
            if exists {
                &[][..]
            } else {
                &["worktree", "marker"][..]
            }
        );
    }
}

#[test]
fn an_unborn_head_and_a_missing_subdirectory_stop_create_before_any_write() {
    let workspace = workspace();
    for unborn in [true, false] {
        let result =
            run(Plan::create(&workspace.original, SESSION, None), |step| {
                match step {
                    Step::Git { ref args, .. } => {
                        let words =
                            args.iter().map(String::as_str).collect::<Vec<_>>();
                        match words.as_slice() {
                            ["rev-parse", "--show-toplevel"] => {
                                output("/repo\n")
                            }
                            ["rev-parse", "--verify", "HEAD^{commit}"]
                                if unborn =>
                            {
                                failed("unborn")
                            }
                            ["rev-parse", "--verify", "HEAD^{commit}"] => {
                                output("tip\n")
                            }
                            ["cat-file", "-t", "tip:sub"] => output("blob\n"),
                            _ => valid(step),
                        }
                    }
                    other => valid(other),
                }
            });
        assert_eq!(
            result,
            if unborn {
                Err(GitError::NotRepository)
            } else {
                Err(GitError::Failed(String::from(
                    "selected subdirectory must exist in committed HEAD: metadata differs",
                )))
            }
        );
    }
}

#[derive(Clone, Copy)]
enum Snapshot {
    New,
    NoChanges,
    Retried,
    DifferentTree,
    Moved,
}

fn snapshot_step(
    step: Step,
    scenario: Snapshot,
    mutations: &mut Vec<Vec<String>>,
) -> Answer {
    let Step::Git {
        path,
        ref args,
        index,
        identity,
    } = step
    else {
        return valid(step);
    };
    let words = args.iter().map(String::as_str).collect::<Vec<_>>();
    assert_eq!(identity, words.first() == Some(&"commit-tree"));
    assert_eq!(
        index,
        matches!(words.first(), Some(&("read-tree" | "add" | "write-tree")))
    );
    match words.as_slice() {
        ["rev-parse", HEAD] => output("tip\n"),
        [
            "for-each-ref",
            "--format=%(objectname)",
            "--count=1",
            "--points-at=previous",
            PREFIX,
        ] => output("previous\n"),
        ["write-tree"] => output("tree\n"),
        ["rev-parse", "--verify", CHECKPOINT] => match scenario {
            Snapshot::New | Snapshot::NoChanges => failed("missing"),
            _ => output("new\n"),
        },
        ["rev-parse", "tip^{tree}"] => match scenario {
            Snapshot::NoChanges => output("tree\n"),
            _ => output("oldtree\n"),
        },
        ["rev-parse", "new^{tree}"] => match scenario {
            Snapshot::DifferentTree => output("wrongtree\n"),
            _ => output("tree\n"),
        },
        [
            "commit-tree",
            "tree",
            "-p",
            "tip",
            "-m",
            "Conductor: checkpoint 7 of the session",
        ] => {
            mutations.push(Vec::from([String::from("commit-tree")]));
            output("new\n")
        }
        ["rev-parse", "--verify", "--quiet", "new^"] => match scenario {
            Snapshot::Moved => output("elsewhere\n"),
            _ => output("tip\n"),
        },
        ["rev-parse", "--verify", "--quiet", "tip^"] => failed("root commit"),
        ["read-tree", "tip"]
        | ["add", "-A", "--", "."]
        | ["update-ref", CHECKPOINT, "new" | "tip", ""]
        | ["update-ref", HEAD, "new", "tip"]
        | ["reset", "--quiet"] => {
            mutations.push(args.clone());
            output("")
        }
        _ => valid(Step::Git {
            path,
            args: args.clone(),
            index,
            identity,
        }),
    }
}

#[test]
fn snapshot_branches_on_git_results_without_dropping_commits_or_rewriting_checkpoints()
 {
    let workspace = workspace();
    for (scenario, expected, writes) in [
        (
            Snapshot::New,
            Ok("new"),
            &[
                "read-tree",
                "add",
                "commit-tree",
                "update-ref",
                "update-ref",
                "reset",
            ][..],
        ),
        (
            Snapshot::NoChanges,
            Ok("tip"),
            &["read-tree", "add", "update-ref"][..],
        ),
        (
            Snapshot::Retried,
            Ok("new"),
            &["read-tree", "add", "update-ref", "reset"][..],
        ),
        (
            Snapshot::DifferentTree,
            Err(
                "checkpoint id already records a different tree: metadata differs",
            ),
            &["read-tree", "add"][..],
        ),
        (
            Snapshot::Moved,
            Err(
                "checkpoint: the session's branch moved on since the checkpoint was taken",
            ),
            &["read-tree", "add"][..],
        ),
    ] {
        let mut mutations = Vec::new();
        let result = run(
            Plan::snapshot(
                &workspace,
                SESSION,
                CheckpointId(7),
                Some("previous"),
            ),
            |step| snapshot_step(step, scenario, &mut mutations),
        );
        assert_eq!(
            result,
            expected
                .map(String::from)
                .map_err(|text| GitError::Failed(String::from(text)))
        );
        assert_eq!(
            mutations
                .iter()
                .map(|args| args[0].as_str())
                .collect::<Vec<_>>(),
            writes
        );
    }
}

#[test]
fn restore_validates_the_commit_before_changing_the_index_or_worktree() {
    let workspace = workspace();
    for owned in [true, false] {
        let mut writes = Vec::new();
        let result = run(Plan::restore(&workspace, "target"), |step| {
            if let Step::Git {
                ref args,
                index: false,
                identity: false,
                ..
            } = step
            {
                let words = args.iter().map(String::as_str).collect::<Vec<_>>();
                match words.as_slice() {
                    [
                        "for-each-ref",
                        "--format=%(objectname)",
                        "--count=1",
                        "--points-at=target",
                        PREFIX,
                    ] => return output(if owned { "target\n" } else { "" }),
                    ["rev-parse", HEAD] => return output("tip\n"),
                    [
                        "restore",
                        "--source=target",
                        "--staged",
                        "--worktree",
                        "--",
                        ".",
                    ]
                    | ["clean", "-q", "-f", "-d", "--", "."]
                    | ["update-ref", HEAD, "target", "tip"]
                    | ["reset", "--quiet"] => {
                        writes.extend(words.first().copied().map(String::from));
                        return output("");
                    }
                    _ => {}
                }
            }
            valid(step)
        });
        if owned {
            assert_eq!(result, Ok(()));
            assert_eq!(writes, ["restore", "clean", "update-ref", "reset"]);
        } else {
            assert!(result.is_err());
            assert_eq!(writes, [] as [String; 0]);
        }
    }
}

#[test]
fn a_foreign_ownership_marker_stops_restore_before_checkpoint_lookup() {
    let workspace = workspace();
    let result = run(Plan::restore(&workspace, "target"), |step| match step {
        Step::ReadOwner(_) => Answer::Text(String::from("foreign")),
        other => valid(other),
    });
    assert_eq!(
        result,
        Err(GitError::Failed(String::from(
            "ownership metadata: metadata differs"
        )))
    );
}

#[test]
fn removal_prunes_a_missing_worktree_even_when_the_push_fails() {
    let workspace = workspace();
    let mut writes = Vec::new();
    let result = run(Plan::remove(&workspace), |step| {
        if let Step::Git { ref args, .. } = step {
            let words = args.iter().map(String::as_str).collect::<Vec<_>>();
            return match words.as_slice() {
                [
                    "rev-parse",
                    "refs/heads/conductor/00000000000000000000000001^{tree}",
                ] => output("tree\n"),
                ["remote", "get-url", "origin"] => output("remote\n"),
                ["push", "--quiet", "origin", refspec]
                    if *refspec == format!("{HEAD}:{HEAD}") =>
                {
                    writes.push("push");
                    failed("rejected")
                }
                ["worktree", "prune"] => {
                    writes.push("prune");
                    output("")
                }
                [
                    "for-each-ref",
                    "--count=500",
                    "--format=%(refname)",
                    PREFIX,
                ] if writes.last() == Some(&"prune") => output(CHECKPOINT),
                ["update-ref", "-d", CHECKPOINT] => {
                    writes.push("delete");
                    output("")
                }
                [
                    "for-each-ref",
                    "--count=500",
                    "--format=%(refname)",
                    PREFIX,
                ] => output(""),
                _ => valid(step),
            };
        }
        match step {
            Step::IsDirectory(ref path) if path == WORKTREE => {
                Answer::Found(false)
            }
            other => valid(other),
        }
    });
    assert_eq!(writes, ["push", "prune", "delete"]);
    assert_eq!(
        result,
        Ok(Kept::PushFailed {
            reason: String::from("push: rejected")
        })
    );
}

fn checkpoint(id: u64, commit: &str) -> Checkpoint {
    Checkpoint {
        id: CheckpointId(id),
        parent: None,
        turn_start: None,
        commit: String::from(commit),
        history: HistoryAnchor::Fresh,
        undo: UndoAvailability::Unavailable {
            reason: String::new(),
        },
    }
}

#[test]
fn diff_checks_immutable_refs_and_applies_the_patch_budget()
-> Result<(), GitError> {
    let workspace = workspace();
    let child = checkpoint(7, "current");
    let parent = checkpoint(6, "previous");
    let patch = "a".repeat(Output::CAPTURE - 1);
    let result = run(Plan::diff(&workspace, &child, &parent), |step| {
        if let Step::Git { ref args, .. } = step {
            let words = args.iter().map(String::as_str).collect::<Vec<_>>();
            match words.as_slice() {
                ["rev-parse", "--verify", CHECKPOINT] => {
                    return output("current\n");
                }
                ["rev-parse", "--verify", reference]
                    if *reference == format!("{PREFIX}6") =>
                {
                    return output("previous\n");
                }
                [
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--no-renames",
                    "--name-status",
                    "-z",
                    previous,
                    CHECKPOINT,
                    "--",
                ] => {
                    assert_eq!(*previous, format!("{PREFIX}6"));
                    return output(b"M\0a\0A\0b\0D\0c\0M\0d\0M\0e\0M\0f\0");
                }
                [
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--no-renames",
                    "--no-color",
                    "previous",
                    "current",
                    "--",
                    name,
                ] => {
                    return if *name == ":(literal)f" {
                        output(b"diff\nBinary files a and b differ\n")
                    } else {
                        assert!(name.starts_with(":(literal)"));
                        output(&patch)
                    };
                }
                _ => {}
            }
        }
        valid(step)
    });
    let diff = result?;
    assert_eq!(diff.extent, DiffExtent::Truncated);
    assert_eq!(diff.files.len(), 6);
    assert_eq!(diff.files[3].patch, FilePatch::Text(patch));
    assert_eq!(diff.files[4].patch, FilePatch::TooLarge);
    assert_eq!(diff.files[5].patch, FilePatch::Binary);
    Ok(())
}

#[derive(Clone, Copy)]
enum Landing {
    AlreadyLanded,
    FastForward,
    Merge,
    CheckedOut,
    Conflict,
    GitError,
}

const ISSUE: &str = "refs/heads/issue/x";

fn landing_step(
    step: Step,
    scenario: Landing,
    writes: &mut Vec<String>,
) -> Answer {
    let Step::Git {
        ref path,
        ref args,
        identity,
        ..
    } = step
    else {
        return match step {
            Step::IsDirectory(ref value) if value == WORKTREE => {
                Answer::Found(false)
            }
            other => valid(other),
        };
    };
    assert_eq!(path, "/repo");
    let words = args.iter().map(String::as_str).collect::<Vec<_>>();
    match words.as_slice() {
        ["rev-parse", reference]
            if *reference == format!("{HEAD}^{{tree}}") =>
        {
            output("tree\n")
        }
        ["remote", "get-url", "origin"] => failed("no origin"),
        ["rev-list", "--count", range]
            if *range == format!("{ISSUE}..{HEAD}") =>
        {
            output(if matches!(scenario, Landing::AlreadyLanded) {
                "0\n"
            } else {
                "2\n"
            })
        }
        ["merge-base", "--is-ancestor", ISSUE, HEAD] => match scenario {
            Landing::Merge | Landing::Conflict | Landing::GitError => {
                failed("not an ancestor")
            }
            _ => output(""),
        },
        ["worktree", "list", "--porcelain"] => match scenario {
            Landing::CheckedOut => output(format!(
                "worktree /repo\nHEAD abc\nbranch refs/heads/main\n\nworktree /repo-issue\nHEAD def\nbranch {ISSUE}\n"
            )),
            _ => output(""),
        },
        [
            "merge-tree",
            "--write-tree",
            "--name-only",
            "--no-messages",
            ISSUE,
            HEAD,
        ] => match scenario {
            Landing::Conflict => Answer::Git(Output {
                bytes: b"tree\na.txt\nb.txt\n".to_vec(),
                success: false,
                error: String::new(),
            }),
            Landing::GitError => failed("fatal: bad revision"),
            _ => output("merged-tree\n"),
        },
        ["rev-parse", ISSUE] => output("old\n"),
        ["rev-parse", HEAD] => output("new\n"),
        [
            "commit-tree",
            "merged-tree",
            "-p",
            "old",
            "-p",
            "new",
            "-m",
            message,
        ] => {
            assert!(identity);
            assert_eq!(*message, format!("Land {HEAD} on {ISSUE}"));
            writes.push(String::from("commit-tree"));
            output("merge\n")
        }
        ["update-ref", ISSUE, commit @ ("new" | "merge"), "old"] => {
            writes.push(String::from(*commit));
            output("")
        }
        _ => valid(step),
    }
}

#[test]
fn landing_fast_forwards_or_merges_and_refuses_to_move_a_checked_out_branch() {
    let workspace = workspace();
    for (scenario, expected, expected_writes) in [
        (Landing::AlreadyLanded, Ok(0), &[][..]),
        (Landing::FastForward, Ok(2), &["new"][..]),
        (Landing::Merge, Ok(2), &["commit-tree", "merge"][..]),
        (
            Landing::CheckedOut,
            Err(
                "the issue's branch is checked out at /repo-issue, so it can't move under it; land the session's branch there",
            ),
            &[][..],
        ),
        (
            Landing::Conflict,
            Err(
                "landing: the session's branch conflicts with the issue's in a.txt, b.txt; merge them by hand",
            ),
            &[][..],
        ),
        (
            Landing::GitError,
            Err(
                "landing: the branches couldn't be merged: fatal: bad revision",
            ),
            &[][..],
        ),
    ] {
        let mut writes = Vec::new();
        let result = run(Plan::land(&workspace, ISSUE), |step| {
            landing_step(step, scenario, &mut writes)
        });
        assert_eq!(
            result,
            expected.map_err(|reason| GitError::Failed(String::from(reason)))
        );
        assert_eq!(writes, expected_writes);
    }
}
