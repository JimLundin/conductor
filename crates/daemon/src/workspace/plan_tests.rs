use core::ops::ControlFlow;

use host::session::SessionDirectory;

use host::session::SessionId;
use host::workspace::Workspace;
use vcs::{DiffExtent, FilePatch, Kept};
use vcs::{Repository, WorkingCopy, Worktree};

use super::super::snapshots;
use super::{Answer, GitError, Plan, Step, head};
use home_files::git::{Instruction, Reply, Revision};
use vcs::plan::kept;

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

fn prepare(base: Option<&str>) -> vcs::command::Prepare {
    vcs::command::Prepare {
        directory: String::from("/repo/sub"),
        name: SESSION.to_string(),
        branch: String::from(BRANCH),
        base: base.map(str::to_owned),
    }
}

fn output(text: &str) -> Answer {
    reply(Reply::Text(String::from(text)))
}

fn reply(value: Reply) -> Answer {
    Answer::Repository(Ok(value))
}

fn failed(error: &str) -> Answer {
    Answer::Repository(Err(GitError::Failed(String::from(error))))
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
        Step::Repository { path, request } => match request {
            Instruction::Root if path == WORKTREE => output(WORKTREE),
            Instruction::HeadReference => output(BRANCH),
            Instruction::CommonDirectory => output("/repo/.git"),
            Instruction::GitDirectory => output(DIRECTORY),
            _ => failed(&format!(
                "unexpected repository step {request:?} in {path}"
            )),
        },
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
        let result =
            run(Plan::create(prepare(Some("issue"))), |step| match step {
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
                Step::Repository { path, request } if path != WORKTREE => {
                    match request {
                        Instruction::ProbeRoot if path == "/repo/sub" => {
                            output("/repo")
                        }
                        Instruction::Resolve(Revision::Commit(reference))
                            if reference == "issue" =>
                        {
                            output("start")
                        }
                        Instruction::Subtree { commit, path } => {
                            assert_eq!(
                                (commit.as_str(), path.as_str()),
                                ("start", "sub")
                            );
                            reply(Reply::Found(true))
                        }
                        Instruction::AddWorktree {
                            branch,
                            path,
                            start,
                        } => {
                            assert_eq!(
                                (
                                    branch.as_str(),
                                    path.as_str(),
                                    start.as_str()
                                ),
                                (BRANCH, WORKTREE, "start")
                            );
                            created.push("worktree");
                            output("")
                        }
                        request => valid(Step::Repository { path, request }),
                    }
                }
                other => valid(other),
            });
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
        let result = run(Plan::create(prepare(None)), |step| match step {
            Step::Repository { ref request, .. } => match request {
                Instruction::ProbeRoot => output("/repo"),
                Instruction::CommittedHead if unborn => {
                    Answer::Repository(Err(GitError::NotRepository))
                }
                Instruction::CommittedHead => output("tip"),
                Instruction::Subtree { commit, path } => {
                    assert_eq!(
                        (commit.as_str(), path.as_str()),
                        ("tip", "sub")
                    );
                    reply(Reply::Found(false))
                }
                _ => valid(step),
            },
            other => valid(other),
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

#[test]
fn removal_prunes_a_missing_worktree_even_when_the_push_fails() {
    let workspace = workspace();
    let mut writes = Vec::new();
    let result = run(
        Plan::remove(vcs::command::PreserveAndRemove {
            workspace: host::workspace::ownership(&workspace),
        }),
        |step| {
            if let Step::Repository { ref request, .. } = step {
                return match request {
                    Instruction::Resolve(Revision::Tree(reference))
                        if reference == HEAD =>
                    {
                        output("tree")
                    }
                    Instruction::Origin { push: true } => output(
                        "https://user:secret@push.example/repo?token=secret",
                    ),
                    Instruction::Push { branch } if branch == HEAD => {
                        writes.push("push");
                        failed(
                            "rejected https://user:secret@push.example/repo?token=secret",
                        )
                    }
                    Instruction::PruneWorktrees => {
                        writes.push("prune");
                        output("")
                    }
                    Instruction::References { prefix, limit: 500 }
                        if prefix.starts_with("refs/conductor-landed/") =>
                    {
                        reply(Reply::References(Vec::new()))
                    }
                    _ => valid(step),
                };
            }
            match step {
                Step::IsDirectory(ref path) if path == WORKTREE => {
                    Answer::Found(false)
                }
                other => valid(other),
            }
        },
    );
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
        let Step::Repository { ref request, .. } = step else {
            return valid(step);
        };
        match request {
            Instruction::Origin { push: true } => output(
                "https://user:password@push.example/repo?token=secret#fragment",
            ),
            Instruction::Push { branch } if branch == HEAD => output(""),
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
    let patch = "a".repeat(vcs::plan::PATCH_LIMIT);
    let result = run(
        Plan::diff(vcs::command::Compare {
            workspace: host::workspace::ownership(&workspace),
            revision: String::from(CURRENT),
            baseline: String::from(PREVIOUS),
        }),
        |step| {
            if let Step::Repository { ref request, .. } = step {
                match request {
                    Instruction::Resolve(Revision::Named(reference))
                        if *reference == format!("{PREFIX}{CURRENT}") =>
                    {
                        return output(CURRENT);
                    }
                    Instruction::Resolve(Revision::Named(reference))
                        if *reference == format!("{PREFIX}{PREVIOUS}") =>
                    {
                        return output(PREVIOUS);
                    }
                    Instruction::DiffNames { previous, commit } => {
                        assert_eq!(
                            (previous.as_str(), commit.as_str()),
                            (PREVIOUS, CURRENT)
                        );
                        return reply(Reply::Names {
                            entries: [
                                ('a', vcs::DiffChange::Edit),
                                ('b', vcs::DiffChange::Write),
                                ('c', vcs::DiffChange::Delete),
                                ('d', vcs::DiffChange::Edit),
                                ('e', vcs::DiffChange::Edit),
                                ('f', vcs::DiffChange::Edit),
                            ]
                            .into_iter()
                            .map(|(path, change)| vcs::Entry {
                                path: path.to_string(),
                                change,
                            })
                            .collect(),
                            extent: DiffExtent::Complete,
                        });
                    }
                    Instruction::DiffPatch {
                        previous,
                        commit,
                        path,
                    } => {
                        assert_eq!(
                            (previous.as_str(), commit.as_str()),
                            (PREVIOUS, CURRENT)
                        );
                        return reply(Reply::Patch(if path == "f" {
                            FilePatch::Binary
                        } else {
                            FilePatch::Text(patch.clone())
                        }));
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
        Step::Repository {
            path: repository,
            request:
                Instruction::Integrate {
                    branch: issue,
                    commit,
                    expected: old,
                    receipt,
                },
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
                    failed(
                        "update-ref: the issue ref is at advanced but expected old",
                    )
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
    let Step::Repository {
        ref path,
        ref request,
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
    match request {
        Instruction::Resolve(Revision::Tree(reference))
            if reference == HEAD =>
        {
            output("tree")
        }
        Instruction::CountAhead { base, tip } => {
            assert_eq!((base.as_str(), tip.as_str()), ("old", "new"));
            reply(Reply::Count(
                if matches!(scenario, Landing::AlreadyLanded) {
                    0
                } else {
                    2
                },
            ))
        }
        Instruction::Ancestor { base, tip } => {
            assert_eq!((base.as_str(), tip.as_str()), ("old", "new"));
            reply(Reply::Found(!matches!(
                scenario,
                Landing::Merge
                    | Landing::AdvancedMerge
                    | Landing::Conflict
                    | Landing::GitError
            )))
        }
        Instruction::Worktrees => reply(Reply::Worktrees(
            if matches!(scenario, Landing::CheckedOut) {
                vec![vcs::Checkout {
                    path: String::from("/repo-issue"),
                    branch: Some(String::from(ISSUE)),
                }]
            } else {
                Vec::new()
            },
        )),
        Instruction::Merge { base, tip } => {
            assert_eq!((base.as_str(), tip.as_str()), ("old", "new"));
            match scenario {
                Landing::Conflict => {
                    reply(Reply::Merge(vcs::Merge::Conflicts(vec![
                        String::from("a.txt"),
                        String::from("b.txt"),
                    ])))
                }
                Landing::GitError => failed(
                    "landing: the branches couldn't be merged: fatal: bad revision",
                ),
                _ => reply(Reply::Merge(vcs::Merge::Tree(String::from(
                    "merged-tree",
                )))),
            }
        }
        Instruction::Resolve(Revision::Named(reference))
            if reference == ISSUE =>
        {
            output("old")
        }
        Instruction::Resolve(Revision::Named(reference))
            if reference == HEAD =>
        {
            output("new")
        }
        Instruction::Commit {
            tree,
            parents,
            message,
        } => {
            assert_eq!(tree, "merged-tree");
            assert_eq!(parents, &["old", "new"]);
            assert_eq!(*message, format!("Land {HEAD} on {ISSUE}"));
            writes.push(String::from("commit-tree"));
            output("merge")
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
            Plan::land(vcs::command::Integrate {
                workspace: host::workspace::ownership(&workspace),
                target: String::from(ISSUE),
                attempt: vcs::command::IntegrationId(String::from("42")),
            }),
            |step| landing_step(step, scenario, &mut writes),
        );
        assert_eq!(
            result,
            expected.map_err(|reason| GitError::Failed(String::from(reason)))
        );
        assert_eq!(writes, expected_writes);
    }
}
