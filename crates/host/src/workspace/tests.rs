use core::ops::ControlFlow;

use crate::session::SessionDirectory;

use crate::log::HostSeq;
use crate::session::SessionId;
use crate::workspace::{DiffExtent, FilePatch, Kept, Workspace};
use crate::workspace::{Repository, WorkingCopy, Worktree};

use super::{Answer, GitError, Output, Plan, Step, head, kept, snapshots};

const SESSION: SessionId = SessionId(1);
const WORKTREE: &str = "/data/worktrees/00000000000000000000000001";
const BRANCH: &str = "conductor/00000000000000000000000001";
const HEAD: &str = "refs/heads/conductor/00000000000000000000000001";
const DIRECTORY: &str = "/repo/.git/worktrees/00000000000000000000000001";
const CURRENT: &str = "1111111111111111111111111111111111111111";
const PREVIOUS: &str = "0000000000000000000000000000000000000000";
const PREFIX: &str = "refs/conductor/00000000000000000000000001/";

fn workspace() -> Workspace {
    Workspace {
        repository: Repository(String::from("/repo")),
        original: WorkingCopy(String::from("/repo/sub")),
        worktree: Worktree(String::from(WORKTREE)),
        directory: SessionDirectory(format!("{WORKTREE}/sub")),
        branch: String::from(BRANCH),
    }
}

fn started_in() -> SessionDirectory {
    SessionDirectory(String::from("/repo/sub"))
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
    assert_eq!(snapshots(&workspace).as_deref(), Ok(PREFIX));
    for exists in [false, true] {
        let mut created = Vec::new();
        let result = run(
            Plan::create(&started_in(), SESSION, Some("issue")),
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
    for unborn in [true, false] {
        let result =
            run(
                Plan::create(&started_in(), SESSION, None),
                |step| match step {
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
                },
            );
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
                ["remote", "get-url", "--push", "origin"] => output(
                    "https://user:secret@push.example/repo?token=secret\n",
                ),
                ["push", "--quiet", "origin", refspec]
                    if *refspec == format!("{HEAD}:{HEAD}") =>
                {
                    writes.push("push");
                    failed(
                        "rejected https://user:secret@push.example/repo?token=secret",
                    )
                }
                ["worktree", "prune"] => {
                    writes.push("prune");
                    output("")
                }
                [
                    "for-each-ref",
                    "--count=500",
                    "--format=%(refname)",
                    prefix,
                ] if prefix.starts_with("refs/conductor-landed/") => output(""),
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
    assert_eq!(writes, ["push", "prune"]);
    assert_eq!(
        result,
        Ok(Kept::PushFailed {
            reason: String::from(
                "Git could not push to origin; the branch remains local"
            )
        })
    );
}

#[test]
fn a_successful_push_reports_only_the_sanitized_push_destination() {
    let workspace = workspace();
    let pushed = run(Plan::push(&workspace, HEAD), |step| {
        let Step::Git { ref args, .. } = step else {
            return valid(step);
        };
        match args
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .as_slice()
        {
            ["remote", "get-url", "--push", "origin"] => output(
                "https://user:password@push.example/repo?token=secret#fragment",
            ),
            ["push", "--quiet", "origin", refspec]
                if *refspec == format!("{HEAD}:{HEAD}") =>
            {
                output("")
            }
            _ => valid(step),
        }
    });
    assert_eq!(
        pushed.map(kept),
        Ok(Kept::Pushed {
            remote: String::from("https://push.example/repo")
        })
    );
}

#[test]
fn diff_checks_immutable_refs_and_applies_the_patch_budget()
-> Result<(), GitError> {
    let workspace = workspace();
    let patch = "a".repeat(Output::CAPTURE - 1);
    let result = run(
        Plan::diff(&workspace, HostSeq(7), CURRENT, PREVIOUS),
        |step| {
            if let Step::Git { ref args, .. } = step {
                let words = args.iter().map(String::as_str).collect::<Vec<_>>();
                match words.as_slice() {
                    ["rev-parse", "--verify", reference]
                        if *reference == format!("{PREFIX}{CURRENT}") =>
                    {
                        return output(CURRENT);
                    }
                    ["rev-parse", "--verify", reference]
                        if *reference == format!("{PREFIX}{PREVIOUS}") =>
                    {
                        return output(PREVIOUS);
                    }
                    [
                        "diff",
                        "--no-ext-diff",
                        "--no-textconv",
                        "--no-renames",
                        "--name-status",
                        "-z",
                        previous,
                        CURRENT,
                        "--",
                    ] => {
                        assert_eq!(*previous, PREVIOUS);
                        return output(b"M\0a\0A\0b\0D\0c\0M\0d\0M\0e\0M\0f\0");
                    }
                    [
                        "diff",
                        "--no-ext-diff",
                        "--no-textconv",
                        "--no-renames",
                        "--no-color",
                        PREVIOUS,
                        CURRENT,
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
        },
    );
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
    AdvancedFastForward,
    AdvancedMerge,
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
    match step {
        Step::LandRefs {
            repository,
            issue,
            commit,
            old,
            receipt,
        } => {
            assert_eq!(repository, "/repo");
            assert_eq!(issue, ISSUE);
            assert_eq!(old, "old");
            assert_eq!(
                receipt,
                format!(
                    "refs/conductor-landed/00000000000000000000000001/42/{ISSUE}/2"
                )
            );
            writes.push(commit);
            match scenario {
                Landing::AdvancedFastForward | Landing::AdvancedMerge => {
                    failed("the issue ref is at advanced but expected old")
                }
                _ => output(""),
            }
        }
        other => landing_git(other, scenario, writes),
    }
}

fn landing_git(
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
        ["rev-list", "--count", "old..new"] => {
            output(if matches!(scenario, Landing::AlreadyLanded) {
                "0\n"
            } else {
                "2\n"
            })
        }
        ["merge-base", "--is-ancestor", "old", "new"] => match scenario {
            Landing::Merge
            | Landing::AdvancedMerge
            | Landing::Conflict
            | Landing::GitError => failed("not an ancestor"),
            _ => output(""),
        },
        ["worktree", "list", "--porcelain", "-z"] => match scenario {
            Landing::CheckedOut => output(format!(
                "worktree /repo\0HEAD abc\0branch refs/heads/main\0\0worktree /repo-issue\0HEAD def\0branch {ISSUE}\0"
            )),
            _ => output(""),
        },
        [
            "merge-tree",
            "--write-tree",
            "--name-only",
            "--no-messages",
            "old",
            "new",
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
            Landing::AdvancedFastForward,
            Err("update-ref: the issue ref is at advanced but expected old"),
            &["new"][..],
        ),
        (
            Landing::AdvancedMerge,
            Err("update-ref: the issue ref is at advanced but expected old"),
            &["commit-tree", "merge"][..],
        ),
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
        let result = run(
            Plan::land(&workspace, ISSUE, crate::log::HostSeq(42)),
            |step| landing_step(step, scenario, &mut writes),
        );
        assert_eq!(
            result,
            expected.map_err(|reason| GitError::Failed(String::from(reason)))
        );
        assert_eq!(writes, expected_writes);
    }
}
