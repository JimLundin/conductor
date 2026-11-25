use core::{
    future::{Future, poll_fn},
    ops::ControlFlow,
    pin::Pin,
    task::{ContextBuilder, Poll, Waker},
};
use std::{boxed::Box, format};

use super::{failure, head};
use home_files::git::{Index, Instruction, Reply, Revision};
use host::workspace::Workspace;
use host::workspace::{owner, ownership};
use vcs::Error as GitError;
use vcs::command::IntegrationId;
use vcs::plan::{Integration, Trees, checked_out, integration};

fn same(actual: &str, expected: &str, context: &str) -> Result<(), GitError> {
    (actual == expected)
        .then_some(())
        .ok_or_else(|| failure(context, "metadata differs"))
}

fn host_path(path: Option<&str>) -> Result<String, GitError> {
    let text = path.ok_or_else(|| {
        failure("Git workspace path", "non-UTF-8 paths are unsupported")
    })?;
    if text.contains('\n') {
        Err(failure(
            "Git workspace path",
            "newline paths are unsupported",
        ))
    } else {
        Ok(String::from(text))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Repository {
        path: String,
        request: Instruction,
    },
    Canonicalize(String),
    Relative {
        path: String,
        root: String,
        context: &'static str,
    },
    Join {
        root: String,
        suffix: String,
    },
    Parent(String),
    Within {
        path: String,
        root: String,
    },
    IsDirectory(String),
    ReadOwner(String),
    OpenIndex(String),
    CloseIndex,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Answer {
    Repository(Result<Reply, GitError>),
    Path(Option<String>),
    Text(String),
    Found(bool),
    Done,
}

#[derive(Default)]
struct Exchange {
    step: Option<Step>,
    answer: Option<Result<Answer, GitError>>,
}

struct Steps;

type StepScript<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, GitError>> + Send + 'a>>;

pub struct Plan<'a, T> {
    script: StepScript<'a, T>,
}

impl<'a, T> Plan<'a, T> {
    fn new<F>(script: impl FnOnce(Steps) -> F) -> Self
    where
        F: Future<Output = Result<T, GitError>> + Send + 'a,
    {
        Self {
            script: Box::pin(script(Steps)),
        }
    }

    pub fn advance(
        mut self,
        answer: Option<Result<Answer, GitError>>,
    ) -> ControlFlow<Result<T, GitError>, (Step, Self)> {
        let mut exchange = Exchange { step: None, answer };
        let mut context = ContextBuilder::from_waker(Waker::noop())
            .ext(&mut exchange)
            .build();
        match self.script.as_mut().poll(&mut context) {
            Poll::Ready(result) => ControlFlow::Break(result),
            Poll::Pending => exchange.step.map_or_else(
                || {
                    ControlFlow::Break(Err(failure(
                        "workspace plan",
                        "missing step",
                    )))
                },
                |step| ControlFlow::Continue((step, self)),
            ),
        }
    }
}

impl Steps {
    async fn perform(&self, step: Step) -> Result<Answer, GitError> {
        let mut request = Some(step);
        poll_fn(|context| {
            let Some(exchange) = context.ext().downcast_mut::<Exchange>()
            else {
                return Poll::Ready(Err(failure(
                    "workspace plan",
                    "missing exchange",
                )));
            };
            match request.take() {
                Some(step) => {
                    exchange.step = Some(step);
                    Poll::Pending
                }
                None => {
                    Poll::Ready(exchange.answer.take().unwrap_or_else(|| {
                        Err(failure("workspace plan", "missing step result"))
                    }))
                }
            }
        })
        .await
    }

    async fn path(&self, step: Step) -> Result<String, GitError> {
        match self.perform(step).await? {
            Answer::Path(path) => host_path(path.as_deref()),
            _ => Err(wrong_answer()),
        }
    }

    async fn found(&self, step: Step) -> Result<bool, GitError> {
        match self.perform(step).await? {
            Answer::Found(found) => Ok(found),
            _ => Err(wrong_answer()),
        }
    }

    async fn done(&self, step: Step) -> Result<(), GitError> {
        match self.perform(step).await? {
            Answer::Done => Ok(()),
            _ => Err(wrong_answer()),
        }
    }

    async fn request(
        &self,
        path: &str,
        request: Instruction,
    ) -> Result<Reply, GitError> {
        match self
            .perform(Step::Repository {
                path: String::from(path),
                request,
            })
            .await?
        {
            Answer::Repository(result) => result,
            _ => Err(wrong_answer()),
        }
    }

    async fn text(
        &self,
        path: &str,
        request: Instruction,
    ) -> Result<String, GitError> {
        match self.request(path, request).await? {
            Reply::Text(text) => Ok(text),
            _ => Err(wrong_answer()),
        }
    }

    async fn references(
        &self,
        path: &str,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<String>, GitError> {
        match self
            .request(
                path,
                Instruction::References {
                    prefix: String::from(prefix),
                    limit,
                },
            )
            .await?
        {
            Reply::References(refs) => Ok(refs),
            _ => Err(wrong_answer()),
        }
    }

    async fn canonical(&self, path: &str) -> Result<String, GitError> {
        self.path(Step::Canonicalize(String::from(path))).await
    }

    async fn join(&self, root: &str, suffix: &str) -> Result<String, GitError> {
        self.path(Step::Join {
            root: String::from(root),
            suffix: String::from(suffix),
        })
        .await
    }

    async fn relative(
        &self,
        path: &str,
        root: &str,
        context: &'static str,
    ) -> Result<String, GitError> {
        self.path(Step::Relative {
            path: String::from(path),
            root: String::from(root),
            context,
        })
        .await
    }

    async fn git_dir(&self, path: &str) -> Result<String, GitError> {
        self.canonical(&self.text(path, Instruction::GitDirectory).await?)
            .await
    }
}

fn wrong_answer() -> GitError {
    failure("workspace plan", "unexpected step result")
}

impl Steps {
    async fn validate(
        &self,
        workspace: &Workspace,
    ) -> Result<String, GitError> {
        let path = &workspace.worktree.0;
        let root = self.canonical(path).await?;
        let repository = self.canonical(&workspace.repository.0).await?;
        let relative = self
            .relative(
                &workspace.original.0,
                &repository,
                "original directory outside repository",
            )
            .await?;
        let session = owner(workspace)?;
        let parent = self.path(Step::Parent(root.clone())).await?;
        let expected = self.join(&parent, &session).await?;
        if let Some(error) = ownership(workspace)
            .paths(&root, &self.join(&root, &relative).await?)
        {
            return Err(error);
        }
        same(&expected, path, "session worktree name")?;
        self.validate_git(workspace, &repository).await
    }

    async fn validate_git(
        &self,
        workspace: &Workspace,
        repository: &str,
    ) -> Result<String, GitError> {
        let directory = self.validate_git_paths(workspace, repository).await?;
        let Answer::Text(recorded) =
            self.perform(Step::ReadOwner(directory.clone())).await?
        else {
            return Err(wrong_answer());
        };
        if !ownership(workspace).matches(&recorded) {
            return Err(failure("ownership metadata", "metadata differs"));
        }
        Ok(directory)
    }

    async fn validate_git_paths(
        &self,
        workspace: &Workspace,
        repository: &str,
    ) -> Result<String, GitError> {
        let path = &workspace.worktree.0;
        let top = self.text(path, Instruction::Root).await?;
        let branch = self.text(path, Instruction::HeadReference).await?;
        let common = self.common_dir(path).await?;
        let source_common = self.common_dir(repository).await?;
        let directory = self.git_dir(path).await?;
        let linked = self
            .found(Step::Within {
                path: directory.clone(),
                root: self.join(&common, "worktrees").await?,
            })
            .await?;
        let observed = vcs::plan::RepositoryFacts {
            root: top,
            branch,
            common,
            source_common,
            linked,
        };
        ownership(workspace)
            .repository(&observed)
            .map_or(Ok(directory), Err)
    }

    async fn common_dir(&self, path: &str) -> Result<String, GitError> {
        let common = self.text(path, Instruction::CommonDirectory).await?;
        self.canonical(&common).await
    }
}

impl Steps {
    async fn tree(&self, path: &str, base: &str) -> Result<String, GitError> {
        let _ = self
            .text(
                path,
                Instruction::ReadTree {
                    revision: String::from(base),
                    index: Index::Scratch,
                },
            )
            .await?;
        let _ = self
            .text(
                path,
                Instruction::Stage {
                    index: Index::Scratch,
                },
            )
            .await?;
        let tree = self
            .text(
                path,
                Instruction::WriteTree {
                    index: Index::Scratch,
                },
            )
            .await?;
        self.done(Step::CloseIndex).await?;
        Ok(tree)
    }

    async fn commit(
        &self,
        path: &str,
        tree: &str,
        parents: &[&str],
        message: &str,
    ) -> Result<String, GitError> {
        self.text(
            path,
            Instruction::Commit {
                tree: String::from(tree),
                parents: parents
                    .iter()
                    .map(|parent| String::from(*parent))
                    .collect(),
                message: String::from(message),
            },
        )
        .await
    }

    async fn update_branch(
        &self,
        workspace: &Workspace,
        commit: &str,
        from: &str,
    ) -> Result<(), GitError> {
        let path = &workspace.worktree.0;
        self.text(
            path,
            Instruction::UpdateReference {
                reference: head(workspace),
                commit: String::from(commit),
                expected: Some(String::from(from)),
            },
        )
        .await
        .map(drop)
    }
}

impl<'a> Plan<'a, u64> {
    pub fn land(
        workspace: &'a Workspace,
        target: &'a str,
        attempt: IntegrationId,
    ) -> Self {
        Self::new(|steps| async move {
            steps.land(workspace, target, &attempt).await
        })
    }
}

impl<'a> Plan<'a, Option<u64>> {
    pub fn landing_receipt(
        workspace: &'a Workspace,
        target: &'a str,
        attempt: IntegrationId,
    ) -> Self {
        Self::new(|steps| async move {
            steps.landing_receipt(workspace, target, &attempt).await
        })
    }
}

type Push = Option<(String, Result<(), String>)>;

impl<'a> Plan<'a, Push> {
    #[must_use]
    pub fn push(workspace: &'a Workspace, branch: &'a str) -> Self {
        Self::new(|steps| async move {
            Ok(steps.push(&workspace.repository.0, branch).await)
        })
    }
}

impl Steps {
    async fn uncommitted(
        &self,
        workspace: &Workspace,
    ) -> Result<Trees, GitError> {
        let repository = &workspace.repository.0;
        let branch = self
            .text(
                repository,
                Instruction::Resolve(Revision::Tree(head(workspace))),
            )
            .await?;
        let worktree = if self
            .found(Step::IsDirectory(workspace.worktree.0.clone()))
            .await?
        {
            let directory = self.validate(workspace).await?;
            let path = &workspace.worktree.0;
            self.done(Step::OpenIndex(directory)).await?;
            let files = self.tree(path, "HEAD").await?;
            Some(files)
        } else {
            None
        };
        Ok(Trees { branch, worktree })
    }

    async fn keep(
        &self,
        workspace: &Workspace,
        tree: &str,
    ) -> Result<(), GitError> {
        let path = &workspace.worktree.0;
        let parent = self
            .text(path, Instruction::Resolve(Revision::Head))
            .await?;
        let commit = self.commit(path, tree, &[&parent], KEPT).await?;
        self.update_branch(workspace, &commit, &parent).await
    }

    async fn origin(&self, repository: &str) -> Option<String> {
        self.text(repository, Instruction::Origin { push: true })
            .await
            .ok()
    }

    async fn push(&self, repository: &str, branch: &str) -> Push {
        let origin = self.origin(repository).await;
        self.push_to(repository, branch, origin).await
    }

    async fn push_to(
        &self,
        repository: &str,
        branch: &str,
        origin: Option<String>,
    ) -> Push {
        let remote = origin?;
        let pushed = self
            .text(
                repository,
                Instruction::Push {
                    branch: String::from(branch),
                },
            )
            .await
            .map(drop)
            .map_err(|_| {
                String::from(
                    "Git could not push to origin; the branch remains local",
                )
            });
        Some((vcs::remote::display(&remote), pushed))
    }

    async fn land(
        &self,
        workspace: &Workspace,
        issue: &str,
        attempt: &IntegrationId,
    ) -> Result<u64, GitError> {
        let trees = self.uncommitted(workspace).await?;
        let repository = &workspace.repository.0;
        if let Some(tree) = trees.changed() {
            self.keep(workspace, tree).await?;
        }
        let session = head(workspace);
        let old = self
            .text(
                repository,
                Instruction::Resolve(Revision::Named(String::from(issue))),
            )
            .await?;
        let new = self
            .text(
                repository,
                Instruction::Resolve(Revision::Named(session.clone())),
            )
            .await?;
        let ahead = self.count_ahead(repository, &old, &new).await?;
        match integration(ahead, None, false) {
            Integration::Unchanged => Ok(0),
            _ => {
                self.integrate(workspace, issue, attempt, &old, &new, ahead)
                    .await
            }
        }
    }

    async fn integrate(
        &self,
        workspace: &Workspace,
        issue: &str,
        attempt: &IntegrationId,
        old: &str,
        new: &str,
        ahead: u64,
    ) -> Result<u64, GitError> {
        let repository = &workspace.repository.0;
        let session = head(workspace);
        let fast_forward = self.is_ancestor(repository, old, new).await?;
        let listed = self.worktrees(repository).await?;
        let decision =
            integration(ahead, checked_out(&listed, issue), fast_forward);
        if let Integration::CheckedOut(path) = &decision {
            return Err(GitError::Failed(format!(
                "the issue's branch is checked out at {path}, so it can't move under it; land the session's branch there"
            )));
        }
        let commit = self
            .landing_commit(
                repository,
                (issue, old),
                (&session, new),
                &decision,
            )
            .await?;
        let prefix = landing_attempt(workspace, issue, attempt)?;
        self.text(
            repository,
            Instruction::Integrate {
                branch: String::from(issue),
                commit,
                expected: String::from(old),
                receipt: format!("{prefix}{ahead}"),
            },
        )
        .await
        .map(|_| ahead)
    }

    async fn count_ahead(
        &self,
        repository: &str,
        base: &str,
        tip: &str,
    ) -> Result<u64, GitError> {
        match self
            .request(
                repository,
                Instruction::CountAhead {
                    base: String::from(base),
                    tip: String::from(tip),
                },
            )
            .await?
        {
            Reply::Count(count) => Ok(count),
            _ => Err(wrong_answer()),
        }
    }

    async fn is_ancestor(
        &self,
        repository: &str,
        base: &str,
        tip: &str,
    ) -> Result<bool, GitError> {
        match self
            .request(
                repository,
                Instruction::Ancestor {
                    base: String::from(base),
                    tip: String::from(tip),
                },
            )
            .await?
        {
            Reply::Found(ancestor) => Ok(ancestor),
            _ => Err(wrong_answer()),
        }
    }

    async fn worktrees(
        &self,
        repository: &str,
    ) -> Result<Vec<vcs::Checkout>, GitError> {
        match self.request(repository, Instruction::Worktrees).await? {
            Reply::Worktrees(trees) => Ok(trees),
            _ => Err(wrong_answer()),
        }
    }

    async fn merge(
        &self,
        repository: &str,
        base: &str,
        tip: &str,
    ) -> Result<vcs::Merge, GitError> {
        match self
            .request(
                repository,
                Instruction::Merge {
                    base: String::from(base),
                    tip: String::from(tip),
                },
            )
            .await?
        {
            Reply::Merge(merge) => Ok(merge),
            _ => Err(wrong_answer()),
        }
    }

    async fn landing_commit(
        &self,
        repository: &str,
        (issue, old): (&str, &str),
        (session, new): (&str, &str),
        decision: &Integration,
    ) -> Result<String, GitError> {
        if *decision != Integration::Merge {
            return Ok(String::from(new));
        }
        let merged = self.merge(repository, old, new).await?;
        let tree = match merged {
            vcs::Merge::Tree(tree) => tree,
            vcs::Merge::Conflicts(paths) => {
                return Err(failure(
                    "landing",
                    &format!(
                        "the session's branch conflicts with the issue's in {}; merge them by hand",
                        paths.join(", ")
                    ),
                ));
            }
        };
        self.commit(
            repository,
            &tree,
            &[old, new],
            &format!("Land {session} on {issue}"),
        )
        .await
    }

    async fn landing_receipt(
        &self,
        workspace: &Workspace,
        issue: &str,
        attempt: &IntegrationId,
    ) -> Result<Option<u64>, GitError> {
        let prefix = landing_attempt(workspace, issue, attempt)?;
        let refs = self.references(&workspace.repository.0, &prefix, 2).await?;
        if refs.is_empty() {
            return Ok(None);
        }
        refs.first()
            .filter(|_| refs.len() == 1)
            .and_then(|reference| reference.strip_prefix(&prefix))
            .and_then(|count| count.parse().ok())
            .map(Some)
            .ok_or_else(|| failure("landing", "invalid landing receipt"))
    }
}

fn landing_refs(workspace: &Workspace) -> Result<String, GitError> {
    Ok(format!("refs/conductor-landed/{}/", owner(workspace)?))
}

fn landing_attempt(
    workspace: &Workspace,
    issue: &str,
    attempt: &IntegrationId,
) -> Result<String, GitError> {
    Ok(format!(
        "{}{}/{issue}/",
        landing_refs(workspace)?,
        attempt.0
    ))
}

const KEPT: &str = "Conductor: the session's uncommitted changes";

#[cfg(test)]
#[path = "plan_tests.rs"]
mod tests;
