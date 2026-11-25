use core::ops::ControlFlow;

use host::session::SessionDirectory;

use host::workspace::Workspace;
use vcs::{Repository, WorkingCopy, Worktree};

use super::{Answer, GitError, Plan, Step};
use home_files::git::{Instruction, Reply, Revision};
use vcs::{Kept, plan::kept};

const WORKTREE: &str = "/data/worktrees/00000000000000000000000001";
const BRANCH: &str = "conductor/00000000000000000000000001";
const HEAD: &str = "refs/heads/conductor/00000000000000000000000001";
const DIRECTORY: &str = "/repo/.git/worktrees/00000000000000000000000001";

fn workspace() -> Workspace {
    Workspace {
        repository: Repository(String::from("/repo")),
        original: WorkingCopy(String::from("/repo/sub")),
        worktree: Worktree(String::from(WORKTREE)),
        directory: SessionDirectory(format!("{WORKTREE}/sub")),
        branch: String::from(BRANCH),
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
        other @ Step::IsDirectory(_) => {
            failed(&format!("unexpected step {other:?}"))
        }
    }
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
            Plan::land(
                &workspace,
                ISSUE,
                vcs::command::IntegrationId(String::from("42")),
            ),
            |step| landing_step(step, scenario, &mut writes),
        );
        assert_eq!(
            result,
            expected.map_err(|reason| GitError::Failed(String::from(reason)))
        );
        assert_eq!(writes, expected_writes);
    }
}
