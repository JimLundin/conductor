use alloc::{boxed::Box, format};
use core::{
    future::{Future, poll_fn},
    ops::ControlFlow,
    pin::Pin,
    task::{ContextBuilder, Poll, Waker},
};

use crate::log::HostSeq;
use crate::session::{SessionDirectory, SessionId};
use crate::workspace::{
    Kept, Repository, SnapshotDiff, WorkingCopy, Workspace, Worktree,
};

use crate::prelude::*;

use super::{
    GitError, Output, PATCH_LIMIT, Turn, branch, budget, checked, checked_out,
    failure, head, host_path, kept, listing, marker, merge_message, merged,
    owner, patch, probed, reference, same, subtree, text, turn, utf8, worktree,
};

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Git {
        path: String,
        args: Vec<String>,
        index: bool,
        identity: bool,
    },
    LandRefs {
        repository: String,
        issue: String,
        commit: String,
        old: String,
        receipt: String,
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

impl Output {
    pub const CAPTURE_ONE_BYTE_PAST_PATCH_LIMIT: usize = PATCH_LIMIT + 1;
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
        original: &'a SessionDirectory,
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
        original: &SessionDirectory,
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
        if let Some(object) = subtree(start.trim(), &relative) {
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
            repository: Repository(repository),
            original: WorkingCopy(original_path),
            directory: SessionDirectory(self.join(&path, &relative).await?),
            worktree: Worktree(path),
            branch: branch(session),
        };
        let present = self
            .found(Step::Exists(workspace.worktree.0.clone()))
            .await?;
        if !present {
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
        let directory = self.validate_git_paths(workspace, repository).await?;
        let Answer::Text(recorded) =
            self.perform(Step::ReadOwner(directory.clone())).await?
        else {
            return Err(wrong_answer());
        };
        same(&recorded, &marker(workspace), "ownership metadata")?;
        Ok(directory)
    }

    async fn validate_git_paths(
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
        same(&top, path, "worktree root")?;
        same(&branch, &head(workspace), "owned branch")?;
        same(&common, &source_common, "common repository")?;
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
    pub fn snapshot(workspace: &'a Workspace, session: SessionId) -> Self {
        Self::new(|steps| async move {
            let directory = steps.validate(workspace).await?;
            same(
                &workspace.branch,
                &branch(session),
                "session owning workspace",
            )?;
            steps.done(Step::OpenIndex(directory)).await?;
            steps.snapshot(workspace).await
        })
    }

    #[must_use]
    pub fn git_text(path: &'a str, args: &'a [&'a str]) -> Self {
        Self::new(|steps| async move { steps.text(path, args).await })
    }
}

impl Steps {
    async fn owned_commit(
        &self,
        workspace: &Workspace,
        commit: &str,
    ) -> Result<(), GitError> {
        let reference = reference(workspace, commit)?;
        let actual = self
            .text(
                &workspace.worktree.0,
                &["rev-parse", "--verify", &reference],
            )
            .await?;
        same(&actual, commit, "recorded commit ref")
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
                "initializing snapshot index",
            )
            .await?;
        let _ = self
            .scratch_git(path, &["add", "-A", "--", "."], "staging snapshot")
            .await?;
        let output = self
            .scratch_git(path, &["write-tree"], "snapshot tree")
            .await?;
        self.done(Step::CloseIndex).await?;
        utf8(output, "snapshot tree").map(|tree| tree.trim().to_owned())
    }

    async fn snapshot(
        &self,
        workspace: &Workspace,
    ) -> Result<String, GitError> {
        let path = &workspace.worktree.0;
        let tip = self.text(path, &["rev-parse", &head(workspace)]).await?;
        let tree = self.tree(path, &tip).await?;
        let tip_tree = self
            .text(path, &["rev-parse", &format!("{tip}^{{tree}}")])
            .await?;
        self.preserve_index(path, &tip_tree, &tree).await?;
        let commit = match turn(&tip_tree, &tree) {
            Turn::Tip => tip.clone(),
            Turn::Commit => {
                self.commit(
                    path,
                    &tree,
                    &[&tip],
                    "Conductor: snapshot session work",
                )
                .await?
            }
        };
        let reference = reference(workspace, &commit)?;
        let _ = self
            .text(path, &["update-ref", &reference, &commit])
            .await?;
        self.sync_index(workspace, &commit).await?;
        if commit != tip {
            self.update_branch(workspace, &commit, &tip).await?;
        }
        Ok(commit)
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
            checked(output, "creating snapshot commit")?,
            "snapshot commit",
        )
        .map(|text| text.trim().to_owned())
    }

    async fn update_branch(
        &self,
        workspace: &Workspace,
        commit: &str,
        from: &str,
    ) -> Result<(), GitError> {
        let path = &workspace.worktree.0;
        self.text(path, &["update-ref", &head(workspace), commit, from])
            .await
            .map(drop)
    }

    async fn sync_index(
        &self,
        workspace: &Workspace,
        commit: &str,
    ) -> Result<(), GitError> {
        self.text(&workspace.worktree.0, &["read-tree", commit])
            .await
            .map(drop)
    }
}

impl<'a> Plan<'a, SnapshotDiff> {
    #[must_use]
    pub fn diff(
        workspace: &'a Workspace,
        snapshot: HostSeq,
        commit: &'a str,
        previous: &'a str,
    ) -> Self {
        Self::new(|steps| async move {
            steps.diff(workspace, snapshot, commit, previous).await
        })
    }
}

impl Steps {
    async fn diff(
        &self,
        workspace: &Workspace,
        snapshot: HostSeq,
        commit: &str,
        previous: &str,
    ) -> Result<SnapshotDiff, GitError> {
        let _ = self.validate(workspace).await?;
        let path = &workspace.worktree.0;
        self.owned_commit(workspace, commit).await?;
        self.owned_commit(workspace, previous).await?;
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
                    previous,
                    commit,
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
                        previous,
                        commit,
                        "--",
                        &format!(":(literal){}", entry.path),
                    ],
                )
                .await?;
            patched.push(patch(output, entry)?);
        }
        let (files, extent) = budget(patched, counted);
        Ok(SnapshotDiff {
            snapshot,
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
    pub fn land(
        workspace: &'a Workspace,
        issue: &'a str,
        attempt: HostSeq,
    ) -> Self {
        Self::new(
            |steps| async move { steps.land(workspace, issue, attempt).await },
        )
    }
}

impl<'a> Plan<'a, Option<u64>> {
    #[must_use]
    pub fn landing_receipt(
        workspace: &'a Workspace,
        issue: &'a str,
        attempt: HostSeq,
    ) -> Self {
        Self::new(|steps| async move {
            steps.landing_receipt(workspace, issue, attempt).await
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
    async fn remove(&self, workspace: &Workspace) -> Result<Kept, GitError> {
        let repository = &workspace.repository.0;
        let trees = self.uncommitted(workspace, true).await?;
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
        self.forget(repository, &landing_refs(workspace)?).await?;
        Ok(kept(pushed))
    }

    async fn uncommitted(
        &self,
        workspace: &Workspace,
        preserve_index: bool,
    ) -> Result<Trees, GitError> {
        let repository = &workspace.repository.0;
        let reference = format!("{}^{{tree}}", head(workspace));
        let branch = self.text(repository, &["rev-parse", &reference]).await?;
        let worktree = if self
            .found(Step::IsDirectory(workspace.worktree.0.clone()))
            .await?
        {
            let directory = self.validate(workspace).await?;
            let path = &workspace.worktree.0;
            self.done(Step::OpenIndex(directory)).await?;
            let files = self.tree(path, "HEAD").await?;
            if preserve_index {
                self.preserve_index(path, &branch, &files).await?;
            }
            Some(files)
        } else {
            None
        };
        Ok(Trees { branch, worktree })
    }

    async fn preserve_index(
        &self,
        path: &str,
        branch: &str,
        files: &str,
    ) -> Result<(), GitError> {
        let staged = self.text(path, &["write-tree"]).await?;
        if staged != branch && staged != files {
            return Err(failure(
                "keeping workspace changes",
                "the Git index contains changes absent from the files; commit or unstage them and retry",
            ));
        }
        Ok(())
    }

    async fn keep(
        &self,
        workspace: &Workspace,
        tree: &str,
    ) -> Result<(), GitError> {
        let path = &workspace.worktree.0;
        let parent = self.text(path, &["rev-parse", "HEAD"]).await?;
        let commit = self.commit(path, tree, &[&parent], KEPT).await?;
        self.update_branch(workspace, &commit, &parent).await
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
        self.text(repository, &["remote", "get-url", "--push", "origin"])
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
            .map_err(|_| {
                String::from(
                    "Git could not push to origin; the branch remains local",
                )
            });
        Some((push_destination(&remote), pushed))
    }

    async fn land(
        &self,
        workspace: &Workspace,
        issue: &str,
        attempt: HostSeq,
    ) -> Result<u64, GitError> {
        let trees = self.uncommitted(workspace, false).await?;
        let repository = &workspace.repository.0;
        if let Some(tree) = trees.changed() {
            self.keep(workspace, tree).await?;
        }
        let session = head(workspace);
        let old = self.text(repository, &["rev-parse", issue]).await?;
        let new = self.text(repository, &["rev-parse", &session]).await?;
        let count = self
            .text(
                repository,
                &["rev-list", "--count", &format!("{old}..{new}")],
            )
            .await?;
        let ahead = count
            .parse()
            .map_err(|_| failure("landing", "Git counted no commits"))?;
        if ahead == 0 {
            return Ok(0);
        }
        let fast_forward = self
            .text(repository, &["merge-base", "--is-ancestor", &old, &new])
            .await
            .is_ok();
        let listed = self
            .text(repository, &["worktree", "list", "--porcelain", "-z"])
            .await?;
        if let Some(path) = checked_out(&listed, issue) {
            return Err(GitError::Failed(format!(
                "the issue's branch is checked out at {path}, so it can't move under it; land the session's branch there"
            )));
        }
        let commit = self
            .landing_commit(
                repository,
                (issue, &old),
                (&session, &new),
                fast_forward,
            )
            .await?;
        let prefix = landing_attempt(workspace, issue, attempt)?;
        match self
            .perform(Step::LandRefs {
                repository: repository.clone(),
                issue: String::from(issue),
                commit,
                old,
                receipt: format!("{prefix}{ahead}"),
            })
            .await?
        {
            Answer::Git(output) => checked(output, "update-ref").map(|_| ahead),
            _ => Err(wrong_answer()),
        }
    }

    async fn landing_commit(
        &self,
        repository: &str,
        (issue, old): (&str, &str),
        (session, new): (&str, &str),
        fast_forward: bool,
    ) -> Result<String, GitError> {
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
                        old,
                        new,
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
        match merge_tree {
            None => Ok(String::from(new)),
            Some(tree) => {
                self.commit(
                    repository,
                    tree.trim(),
                    &[old, new],
                    &merge_message(issue, session),
                )
                .await
            }
        }
    }

    async fn landing_receipt(
        &self,
        workspace: &Workspace,
        issue: &str,
        attempt: HostSeq,
    ) -> Result<Option<u64>, GitError> {
        let prefix = landing_attempt(workspace, issue, attempt)?;
        let refs = self
            .text(
                &workspace.repository.0,
                &["for-each-ref", "--count=2", "--format=%(refname)", &prefix],
            )
            .await?;
        if refs.is_empty() {
            return Ok(None);
        }
        refs.strip_prefix(&prefix)
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
    attempt: HostSeq,
) -> Result<String, GitError> {
    Ok(format!(
        "{}{}/{issue}/",
        landing_refs(workspace)?,
        attempt.0
    ))
}

fn push_destination(remote: &str) -> String {
    if let Some((scheme, address)) = remote.split_once("://") {
        let destination = address.split(['?', '#']).next().unwrap_or_default();
        let (authority, path) =
            destination.split_once('/').unwrap_or((destination, ""));
        let host = authority.rsplit('@').next().unwrap_or_default();
        return if path.is_empty() {
            format!("{scheme}://{host}")
        } else {
            format!("{scheme}://{host}/{path}")
        };
    }
    if let Some((authority, path)) = remote.split_once(':')
        && let Some((_, host)) = authority.rsplit_once('@')
    {
        return format!("{host}:{path}");
    }
    String::from(remote)
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
