use alloc::{boxed::Box, format, string::ToString};
use core::{
    future::{Future, poll_fn},
    ops::ControlFlow,
    pin::Pin,
    task::{ContextBuilder, Poll, Waker},
};

use proto::{
    Checkpoint, CheckpointDiff, CheckpointId, HostPath, Kept, SessionId,
    Workspace,
};

use crate::prelude::*;

use super::{
    GitError, Output, PATCH_LIMIT, Turn, branch, budget, checked, checked_out,
    checkpoints, existing, failure, head, host_path, kept, listing, marker,
    merge_message, merged, message, owned, owner, patch, probed, reference,
    reuse, same, settle, subtree, text, turn, utf8, worktree,
};

/// An operation requested by a workspace plan.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Git {
        path: String,
        args: Vec<String>,
        index: bool,
        identity: bool,
    },
    Canonicalize(String),
    WorktreeParent,
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
    Exists(String),
    IsDirectory(String),
    ReadOwner(String),
    WriteOwner {
        directory: String,
        marker: String,
    },
    OpenIndex(String),
    CloseIndex,
}

/// The observation made by running a step.
#[derive(Debug, PartialEq, Eq)]
pub enum Answer {
    Git(Output),
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

type Script<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, GitError>> + Send + 'a>>;

/// A workspace workflow. Each response advances it to another operation or
/// a final result.
pub struct Plan<'a, T> {
    script: Script<'a, T>,
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

    /// Start with `None`, then pass the result of each requested step.
    pub fn advance(
        mut self,
        answer: Option<Result<Answer, GitError>>,
    ) -> ControlFlow<Result<T, GitError>, (Step, Self)> {
        let mut exchange = Exchange { step: None, answer };
        let mut context = ContextBuilder::from_waker(Waker::noop())
            .ext(&mut exchange)
            .build();
        // The script only waits on steps, which publish a request before
        // suspending. No executor or wakeups are involved.
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

impl Output {
    /// Read one byte past the limit, so the capture can report truncation.
    pub const CAPTURE: usize = PATCH_LIMIT + 1;
}

impl GitError {
    #[must_use]
    pub fn failed(context: &str, error: &str) -> Self {
        failure(context, error)
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
            Answer::Path(path) => host_path(path.as_deref()).map(|host| host.0),
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

    async fn git_with(
        &self,
        path: &str,
        args: &[&str],
        index: bool,
        identity: bool,
    ) -> Result<Output, GitError> {
        match self
            .perform(Step::Git {
                path: String::from(path),
                args: args.iter().map(|arg| String::from(*arg)).collect(),
                index,
                identity,
            })
            .await?
        {
            Answer::Git(output) => Ok(output),
            _ => Err(wrong_answer()),
        }
    }

    async fn git(&self, path: &str, args: &[&str]) -> Result<Output, GitError> {
        self.git_with(path, args, false, false).await
    }

    async fn text(
        &self,
        path: &str,
        args: &[&str],
    ) -> Result<String, GitError> {
        text(
            self.git(path, args).await?,
            args.first().copied().unwrap_or("git"),
        )
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
        self.canonical(
            &self
                .text(path, &["rev-parse", "--absolute-git-dir"])
                .await?,
        )
        .await
    }
}

fn wrong_answer() -> GitError {
    failure("workspace plan", "unexpected step result")
}

impl<'a> Plan<'a, Workspace> {
    #[must_use]
    pub fn create(
        original: &'a HostPath,
        session: SessionId,
        base: Option<&'a str>,
    ) -> Self {
        Self::new(
            |steps| async move { steps.create(original, session, base).await },
        )
    }
}

impl Steps {
    async fn create(
        &self,
        original: &HostPath,
        session: SessionId,
        base: Option<&str>,
    ) -> Result<Workspace, GitError> {
        let original_path = self.canonical(&original.0).await?;
        let probe = self
            .git(&original_path, &["rev-parse", "--show-toplevel"])
            .await?;
        let root = probed(probe, "repository root")?;
        let repository = self.canonical(root.trim_end_matches('\n')).await?;
        let start = self.start_commit(&repository, base).await?;
        let relative = self
            .relative(&original_path, &repository, "selected subdirectory")
            .await?;
        if let Some(object) = subtree(start.trim(), &HostPath(relative.clone()))
        {
            let kind =
                self.text(&repository, &["cat-file", "-t", &object]).await?;
            same(
                &kind,
                "tree",
                "selected subdirectory must exist in committed HEAD",
            )?;
        }
        let parent = self.path(Step::WorktreeParent).await?;
        let path = self.join(&parent, &worktree(session)).await?;
        let workspace = Workspace {
            repository: HostPath(repository),
            original: HostPath(original_path),
            directory: HostPath(self.join(&path, &relative).await?),
            worktree: HostPath(path),
            branch: branch(session),
        };
        if !self
            .found(Step::Exists(workspace.worktree.0.clone()))
            .await?
        {
            self.add_worktree(&workspace, start.trim()).await?;
        }
        let _ = self.validate(&workspace).await?;
        Ok(workspace)
    }

    async fn start_commit(
        &self,
        repository: &str,
        base: Option<&str>,
    ) -> Result<String, GitError> {
        if let Some(reference) = base {
            self.text(
                repository,
                &["rev-parse", "--verify", &format!("{reference}^{{commit}}")],
            )
            .await
        } else {
            let out = self
                .git(repository, &["rev-parse", "--verify", "HEAD^{commit}"])
                .await?;
            probed(out, "committed HEAD")
        }
    }

    async fn add_worktree(
        &self,
        workspace: &Workspace,
        start: &str,
    ) -> Result<(), GitError> {
        let _ = self
            .text(
                &workspace.repository.0,
                &[
                    "worktree",
                    "add",
                    "-b",
                    &workspace.branch,
                    &workspace.worktree.0,
                    start,
                ],
            )
            .await?;
        self.done(Step::WriteOwner {
            directory: self.git_dir(&workspace.worktree.0).await?,
            marker: marker(workspace),
        })
        .await
    }

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
        same(&root, path, "canonical worktree")?;
        same(
            &workspace.directory.0,
            &self.join(&root, &relative).await?,
            "selected directory metadata",
        )?;
        same(&expected, path, "session worktree name")?;
        self.validate_git(workspace, &repository).await
    }

    async fn validate_git(
        &self,
        workspace: &Workspace,
        repository: &str,
    ) -> Result<String, GitError> {
        let path = &workspace.worktree.0;
        let top = self.text(path, &["rev-parse", "--show-toplevel"]).await?;
        let branch = self.text(path, &["symbolic-ref", "HEAD"]).await?;
        let common = self.common_dir(path).await?;
        let source_common = self.common_dir(repository).await?;
        let directory = self.git_dir(path).await?;
        let Answer::Text(recorded) =
            self.perform(Step::ReadOwner(directory.clone())).await?
        else {
            return Err(wrong_answer());
        };
        same(&top, path, "worktree root")?;
        same(&branch, &head(workspace), "owned branch")?;
        same(&common, &source_common, "common repository")?;
        same(&recorded, &marker(workspace), "ownership metadata")?;
        if self
            .found(Step::Within {
                path: directory.clone(),
                root: self.join(&common, "worktrees").await?,
            })
            .await?
        {
            Ok(directory)
        } else {
            Err(failure(
                "workspace",
                "Git directory is outside repository worktrees",
            ))
        }
    }

    async fn common_dir(&self, path: &str) -> Result<String, GitError> {
        let common = self
            .text(
                path,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )
            .await?;
        self.canonical(&common).await
    }
}

impl<'a> Plan<'a, String> {
    #[must_use]
    pub fn snapshot(
        workspace: &'a Workspace,
        session: SessionId,
        checkpoint: CheckpointId,
        previous: Option<&'a str>,
    ) -> Self {
        Self::new(|steps| async move {
            let directory = steps.validate(workspace).await?;
            same(
                &workspace.branch,
                &branch(session),
                "session owning workspace",
            )?;
            steps.done(Step::OpenIndex(directory)).await?;
            steps.snapshot(workspace, checkpoint, previous).await
        })
    }

    #[must_use]
    pub fn git_text(path: &'a str, args: &'a [&'a str]) -> Self {
        Self::new(|steps| async move { steps.text(path, args).await })
    }
}

impl<'a> Plan<'a, ()> {
    #[must_use]
    pub fn preflight(workspace: &'a Workspace, commit: &'a str) -> Self {
        Self::new(
            |steps| async move { steps.preflight(workspace, commit).await },
        )
    }

    #[must_use]
    pub fn restore(workspace: &'a Workspace, commit: &'a str) -> Self {
        Self::new(|steps| async move { steps.restore(workspace, commit).await })
    }
}

impl Steps {
    async fn owned_commit(
        &self,
        workspace: &Workspace,
        commit: &str,
    ) -> Result<(), GitError> {
        let refs = self
            .text(
                &workspace.worktree.0,
                &[
                    "for-each-ref",
                    "--format=%(objectname)",
                    "--count=1",
                    &format!("--points-at={commit}"),
                    &checkpoints(workspace)?,
                ],
            )
            .await?;
        owned(&refs, commit)
    }

    async fn scratch_git(
        &self,
        path: &str,
        args: &[&str],
        context: &str,
    ) -> Result<Vec<u8>, GitError> {
        checked(self.git_with(path, args, true, false).await?, context)
    }

    async fn tree(&self, path: &str, base: &str) -> Result<String, GitError> {
        let _ = self
            .scratch_git(
                path,
                &["read-tree", base],
                "initializing checkpoint index",
            )
            .await?;
        let _ = self
            .scratch_git(path, &["add", "-A", "--", "."], "staging checkpoint")
            .await?;
        let output = self
            .scratch_git(path, &["write-tree"], "checkpoint tree")
            .await?;
        self.done(Step::CloseIndex).await?;
        utf8(output, "checkpoint tree").map(|tree| tree.trim().to_owned())
    }

    async fn snapshot(
        &self,
        workspace: &Workspace,
        checkpoint: CheckpointId,
        previous: Option<&str>,
    ) -> Result<String, GitError> {
        let path = &workspace.worktree.0;
        if let Some(commit) = previous {
            self.owned_commit(workspace, commit).await?;
        }
        let tip = self.text(path, &["rev-parse", &head(workspace)]).await?;
        let tree = self.tree(path, &tip).await?;
        let commit = self
            .recorded_or_made(workspace, checkpoint, &tip, &tree)
            .await?;
        let parent = self
            .text(
                path,
                &["rev-parse", "--verify", "--quiet", &format!("{commit}^")],
            )
            .await
            .ok();
        if settle(&tip, &commit, parent.as_deref())? {
            self.move_branch(workspace, &commit, &tip).await?;
        }
        Ok(commit)
    }

    async fn recorded_or_made(
        &self,
        workspace: &Workspace,
        checkpoint: CheckpointId,
        tip: &str,
        tree: &str,
    ) -> Result<String, GitError> {
        let path = &workspace.worktree.0;
        let reference = reference(workspace, checkpoint)?;
        let output = self
            .git(path, &["rev-parse", "--verify", &reference])
            .await?;
        if let Some(commit) = existing(output)? {
            let recorded = self
                .text(path, &["rev-parse", &format!("{commit}^{{tree}}")])
                .await?;
            reuse(&recorded, tree)?;
            return Ok(commit);
        }
        let tip_tree = self
            .text(path, &["rev-parse", &format!("{tip}^{{tree}}")])
            .await?;
        let made = match turn(&tip_tree, tree) {
            Turn::Tip => String::from(tip),
            Turn::Commit => {
                self.commit(path, tree, &[tip], &message(checkpoint))
                    .await?
            }
        };
        let _ = self
            .text(path, &["update-ref", &reference, &made, ""])
            .await?;
        Ok(made)
    }

    async fn commit(
        &self,
        path: &str,
        tree: &str,
        parents: &[&str],
        message: &str,
    ) -> Result<String, GitError> {
        let mut args = Vec::from(["commit-tree", tree]);
        for parent in parents {
            args.extend(["-p", parent]);
        }
        args.extend(["-m", message]);
        let output = self.git_with(path, &args, false, true).await?;
        utf8(
            checked(output, "creating checkpoint commit")?,
            "checkpoint commit",
        )
        .map(|text| text.trim().to_owned())
    }

    async fn preflight(
        &self,
        workspace: &Workspace,
        commit: &str,
    ) -> Result<(), GitError> {
        let _ = self.validate(workspace).await?;
        self.owned_commit(workspace, commit).await
    }

    async fn restore(
        &self,
        workspace: &Workspace,
        commit: &str,
    ) -> Result<(), GitError> {
        self.preflight(workspace, commit).await?;
        let path = &workspace.worktree.0;
        let _ = self
            .text(
                path,
                &[
                    "restore",
                    &format!("--source={commit}"),
                    "--staged",
                    "--worktree",
                    "--",
                    ".",
                ],
            )
            .await?;
        let _ = self
            .text(path, &["clean", "-q", "-f", "-d", "--", "."])
            .await?;
        let tip = self.text(path, &["rev-parse", &head(workspace)]).await?;
        self.move_branch(workspace, commit, &tip).await
    }

    async fn move_branch(
        &self,
        workspace: &Workspace,
        commit: &str,
        from: &str,
    ) -> Result<(), GitError> {
        let path = &workspace.worktree.0;
        let _ = self
            .text(path, &["update-ref", &head(workspace), commit, from])
            .await?;
        self.text(path, &["reset", "--quiet"]).await.map(drop)
    }
}

impl<'a> Plan<'a, CheckpointDiff> {
    #[must_use]
    pub fn diff(
        workspace: &'a Workspace,
        checkpoint: &'a Checkpoint,
        parent: &'a Checkpoint,
    ) -> Self {
        Self::new(|steps| async move {
            steps.diff(workspace, checkpoint, parent).await
        })
    }
}

impl Steps {
    async fn diff(
        &self,
        workspace: &Workspace,
        checkpoint: &Checkpoint,
        parent: &Checkpoint,
    ) -> Result<CheckpointDiff, GitError> {
        let _ = self.validate(workspace).await?;
        let path = &workspace.worktree.0;
        let checkpoint_ref = reference(workspace, checkpoint.id)?;
        let parent_ref = reference(workspace, parent.id)?;
        let actual = self
            .text(path, &["rev-parse", "--verify", &checkpoint_ref])
            .await?;
        let actual_parent = self
            .text(path, &["rev-parse", "--verify", &parent_ref])
            .await?;
        same(&actual, &checkpoint.commit, "immutable checkpoint ref")?;
        same(&actual_parent, &parent.commit, "immutable parent ref")?;
        let output = self
            .git(
                path,
                &[
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--no-renames",
                    "--name-status",
                    "-z",
                    &parent_ref,
                    &checkpoint_ref,
                    "--",
                ],
            )
            .await?;
        let (entries, counted) = listing(output)?;
        let mut patched = Vec::with_capacity(entries.len());
        for entry in entries {
            let output = self
                .git(
                    path,
                    &[
                        "diff",
                        "--no-ext-diff",
                        "--no-textconv",
                        "--no-renames",
                        "--no-color",
                        &parent.commit,
                        &checkpoint.commit,
                        "--",
                        &format!(":(literal){}", entry.path),
                    ],
                )
                .await?;
            patched.push(patch(output, entry)?);
        }
        let (files, extent) = budget(patched, counted);
        Ok(CheckpointDiff {
            checkpoint: checkpoint.id,
            parent: parent.id,
            files,
            extent,
        })
    }
}

impl<'a> Plan<'a, Kept> {
    #[must_use]
    pub fn remove(workspace: &'a Workspace) -> Self {
        Self::new(|steps| async move { steps.remove(workspace).await })
    }
}

impl<'a> Plan<'a, u64> {
    #[must_use]
    pub fn land(workspace: &'a Workspace, issue: &'a str) -> Self {
        Self::new(|steps| async move { steps.land(workspace, issue).await })
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
    async fn remove(&self, workspace: &Workspace) -> Result<Kept, GitError> {
        let repository = &workspace.repository.0;
        let trees = self.uncommitted(workspace).await?;
        let origin = self.origin(repository).await;
        let _ = owner(workspace)?;
        if let Some(tree) = trees.changed() {
            self.keep(workspace, tree).await?;
        }
        let pushed = self.push_to(repository, &head(workspace), origin).await;
        if trees.worktree.is_some() {
            let _ = self.validate(workspace).await?;
            let _ = self
                .text(
                    repository,
                    &["worktree", "remove", "--force", &workspace.worktree.0],
                )
                .await?;
        } else {
            let _ = self.text(repository, &["worktree", "prune"]).await?;
        }
        self.forget(repository, &checkpoints(workspace)?).await?;
        Ok(kept(pushed))
    }

    async fn uncommitted(
        &self,
        workspace: &Workspace,
    ) -> Result<Trees, GitError> {
        let repository = &workspace.repository.0;
        let reference = format!("{}^{{tree}}", head(workspace));
        let branch = self.text(repository, &["rev-parse", &reference]).await?;
        let worktree = if self
            .found(Step::IsDirectory(workspace.worktree.0.clone()))
            .await?
        {
            let directory = self.validate(workspace).await?;
            self.done(Step::OpenIndex(directory)).await?;
            Some(self.tree(&workspace.worktree.0, "HEAD").await?)
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
        let parent = self.text(path, &["rev-parse", "HEAD"]).await?;
        let commit = self.commit(path, tree, &[&parent], KEPT).await?;
        self.text(path, &["update-ref", &head(workspace), &commit, &parent])
            .await
            .map(drop)
    }

    async fn forget(
        &self,
        repository: &str,
        prefix: &str,
    ) -> Result<(), GitError> {
        loop {
            let refs = self
                .text(
                    repository,
                    &[
                        "for-each-ref",
                        "--count=500",
                        "--format=%(refname)",
                        prefix,
                    ],
                )
                .await?;
            if refs.is_empty() {
                return Ok(());
            }
            for name in refs.lines() {
                let _ =
                    self.text(repository, &["update-ref", "-d", name]).await?;
            }
        }
    }

    async fn origin(&self, repository: &str) -> Option<String> {
        self.text(repository, &["remote", "get-url", "origin"])
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
        let refspec = format!("{branch}:{branch}");
        let pushed = self
            .text(repository, &["push", "--quiet", "origin", &refspec])
            .await
            .map(drop)
            .map_err(|error| error.to_string());
        Some((remote, pushed))
    }

    async fn land(
        &self,
        workspace: &Workspace,
        issue: &str,
    ) -> Result<u64, GitError> {
        let trees = self.uncommitted(workspace).await?;
        let repository = &workspace.repository.0;
        let _ = self.origin(repository).await;
        if let Some(tree) = trees.changed() {
            self.keep(workspace, tree).await?;
        }
        let session = head(workspace);
        let count = self
            .text(
                repository,
                &["rev-list", "--count", &format!("{issue}..{session}")],
            )
            .await?;
        let fast_forward = self
            .text(
                repository,
                &["merge-base", "--is-ancestor", issue, &session],
            )
            .await
            .is_ok();
        let listed = self
            .text(repository, &["worktree", "list", "--porcelain"])
            .await?;
        let ahead = count
            .trim()
            .parse()
            .map_err(|_| failure("landing", "Git counted no commits"))?;
        if let Some(path) = checked_out(&listed, issue) {
            Err(GitError::Failed(format!(
                "the issue's branch is checked out at {path}, so it can't move under it; land the session's branch there"
            )))
        } else if ahead == 0 {
            Ok(0)
        } else {
            self.land_commits(repository, issue, &session, fast_forward)
                .await?;
            Ok(ahead)
        }
    }

    async fn land_commits(
        &self,
        repository: &str,
        issue: &str,
        session: &str,
        fast_forward: bool,
    ) -> Result<(), GitError> {
        let merge_tree = if fast_forward {
            None
        } else {
            let output = self
                .git(
                    repository,
                    &[
                        "merge-tree",
                        "--write-tree",
                        "--name-only",
                        "--no-messages",
                        issue,
                        session,
                    ],
                )
                .await?;
            Some(
                merged(
                    output.success,
                    &String::from_utf8_lossy(&output.bytes),
                    &output.error,
                )
                .map_err(|reason| failure("landing", &reason))?,
            )
        };
        let old = self.text(repository, &["rev-parse", issue]).await?;
        let new = self.text(repository, &["rev-parse", session]).await?;
        let commit = match merge_tree {
            None => new,
            Some(tree) => {
                self.commit(
                    repository,
                    tree.trim(),
                    &[&old, &new],
                    &merge_message(issue, session),
                )
                .await?
            }
        };
        self.text(repository, &["update-ref", issue, &commit, &old])
            .await
            .map(drop)
    }
}

const KEPT: &str = "Conductor: the session's uncommitted changes";

struct Trees {
    branch: String,
    worktree: Option<String>,
}

impl Trees {
    fn changed(&self) -> Option<&str> {
        self.worktree.as_deref().filter(|tree| *tree != self.branch)
    }
}
