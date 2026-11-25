use core::{
    future::{Future, poll_fn},
    ops::ControlFlow,
    pin::Pin,
    task::{ContextBuilder, Poll, Waker},
};
use std::{boxed::Box, format};

use home_files::git::{Index, Instruction, Reply, Revision};
use host::session::SessionDirectory;
use host::workspace::Workspace;
use vcs::command::{
    Capture, Compare, Comparison, Integrate, IntegrationId, Prepare,
    PreserveAndRemove,
};
use vcs::plan::{
    Integration, Snapshot as Turn, Trees, budget, checked_out, index_preserved,
    integration, kept, snapshot as turn,
};
use vcs::{
    Error as GitError, Kept, Repository, SnapshotFile, WorkingCopy, Worktree,
};

use super::{failure, head, reference};
use host::workspace::{owner, ownership};

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

/// An operation requested by a workspace plan.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Repository {
        path: String,
        request: Instruction,
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

/// A workspace workflow. Each response advances it to another operation or
/// a final result.
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

    /// Start with `None`, then pass the result of each requested step.
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

impl Plan<'_, Workspace> {
    pub fn create(request: Prepare) -> Self {
        Self::new(|steps| async move { steps.create(&request).await })
    }
}

impl Steps {
    async fn create(&self, request: &Prepare) -> Result<Workspace, GitError> {
        let original_path = self.canonical(&request.directory).await?;
        let root = self.text(&original_path, Instruction::ProbeRoot).await?;
        let repository = self.canonical(root.trim_end_matches('\n')).await?;
        let start = self
            .start_commit(&repository, request.base.as_deref())
            .await?;
        let relative = self
            .relative(&original_path, &repository, "selected subdirectory")
            .await?;
        if !relative.is_empty() {
            self.require_directory(&repository, start.trim(), &relative)
                .await?;
        }
        let parent = self.path(Step::WorktreeParent).await?;
        let path = self.join(&parent, &request.name).await?;
        let workspace = Workspace {
            repository: Repository(repository),
            original: WorkingCopy(original_path),
            directory: SessionDirectory(self.join(&path, &relative).await?),
            worktree: Worktree(path),
            branch: request.branch.clone(),
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

    async fn require_directory(
        &self,
        repository: &str,
        commit: &str,
        path: &str,
    ) -> Result<(), GitError> {
        let Reply::Found(directory) = self
            .request(
                repository,
                Instruction::Subtree {
                    commit: String::from(commit),
                    path: String::from(path),
                },
            )
            .await?
        else {
            return Err(wrong_answer());
        };
        if directory {
            Ok(())
        } else {
            Err(failure(
                "selected subdirectory must exist in committed HEAD",
                "metadata differs",
            ))
        }
    }

    async fn start_commit(
        &self,
        repository: &str,
        base: Option<&str>,
    ) -> Result<String, GitError> {
        match base {
            Some(reference) => {
                self.text(
                    repository,
                    Instruction::Resolve(Revision::Commit(String::from(
                        reference,
                    ))),
                )
                .await
            }
            None => self.text(repository, Instruction::CommittedHead).await,
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
                Instruction::AddWorktree {
                    branch: workspace.branch.clone(),
                    path: workspace.worktree.0.clone(),
                    start: String::from(start),
                },
            )
            .await?;
        self.done(Step::WriteOwner {
            directory: self.git_dir(&workspace.worktree.0).await?,
            marker: ownership(workspace).marker(),
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

impl Plan<'_, String> {
    pub fn snapshot(request: Capture) -> Self {
        Self::new(|steps| async move {
            let workspace = workspace(request.workspace);
            let directory = steps.validate(&workspace).await?;
            same(
                &workspace.branch,
                &request.expected_branch,
                "session owning workspace",
            )?;
            steps.done(Step::OpenIndex(directory)).await?;
            steps.snapshot(&workspace).await
        })
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
                Instruction::Resolve(Revision::Named(reference)),
            )
            .await?;
        same(&actual, commit, "recorded commit ref")
    }

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

    async fn snapshot(
        &self,
        workspace: &Workspace,
    ) -> Result<String, GitError> {
        let path = &workspace.worktree.0;
        let tip = self
            .text(path, Instruction::Resolve(Revision::Named(head(workspace))))
            .await?;
        let tree = self.tree(path, &tip).await?;
        let tip_tree = self
            .text(path, Instruction::Resolve(Revision::Tree(tip.clone())))
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
            .text(
                path,
                Instruction::UpdateReference {
                    reference,
                    commit: commit.clone(),
                    expected: None,
                },
            )
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

    async fn sync_index(
        &self,
        workspace: &Workspace,
        commit: &str,
    ) -> Result<(), GitError> {
        self.text(
            &workspace.worktree.0,
            Instruction::ReadTree {
                revision: String::from(commit),
                index: Index::Working,
            },
        )
        .await
        .map(drop)
    }
}

impl Plan<'_, Comparison> {
    pub fn diff(request: Compare) -> Self {
        Self::new(|steps| async move {
            steps
                .diff(
                    &workspace(request.workspace),
                    &request.revision,
                    &request.baseline,
                )
                .await
        })
    }
}

impl Steps {
    async fn diff(
        &self,
        workspace: &Workspace,
        commit: &str,
        previous: &str,
    ) -> Result<Comparison, GitError> {
        let _ = self.validate(workspace).await?;
        let path = &workspace.worktree.0;
        self.owned_commit(workspace, commit).await?;
        self.owned_commit(workspace, previous).await?;
        let Reply::Names {
            entries,
            extent: counted,
        } = self
            .request(
                path,
                Instruction::DiffNames {
                    previous: String::from(previous),
                    commit: String::from(commit),
                },
            )
            .await?
        else {
            return Err(wrong_answer());
        };
        let mut patched = Vec::with_capacity(entries.len());
        for entry in entries {
            patched.push(self.patch(path, previous, commit, entry).await?);
        }
        let (files, extent) = budget(patched, counted);
        Ok(Comparison { files, extent })
    }
    async fn patch(
        &self,
        root: &str,
        previous: &str,
        commit: &str,
        entry: vcs::Entry,
    ) -> Result<SnapshotFile, GitError> {
        let Reply::Patch(patch) = self
            .request(
                root,
                Instruction::DiffPatch {
                    previous: String::from(previous),
                    commit: String::from(commit),
                    path: entry.path.clone(),
                },
            )
            .await?
        else {
            return Err(wrong_answer());
        };
        Ok(SnapshotFile {
            path: entry.path,
            change: entry.change,
            patch,
        })
    }
}

impl Plan<'_, Kept> {
    pub fn remove(request: PreserveAndRemove) -> Self {
        Self::new(|steps| async move {
            steps.remove(&workspace(request.workspace)).await
        })
    }
}

impl Plan<'_, u64> {
    pub fn land(request: Integrate) -> Self {
        Self::new(|steps| async move {
            steps
                .land(
                    &workspace(request.workspace),
                    &request.target,
                    &request.attempt,
                )
                .await
        })
    }
}

impl Plan<'_, Option<u64>> {
    pub fn landing_receipt(request: Integrate) -> Self {
        Self::new(|steps| async move {
            steps
                .landing_receipt(
                    &workspace(request.workspace),
                    &request.target,
                    &request.attempt,
                )
                .await
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
                    Instruction::RemoveWorktree {
                        path: workspace.worktree.0.clone(),
                    },
                )
                .await?;
        } else {
            let _ = self.text(repository, Instruction::PruneWorktrees).await?;
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
        let staged = self
            .text(
                path,
                Instruction::WriteTree {
                    index: Index::Working,
                },
            )
            .await?;
        if !index_preserved(branch, files, &staged) {
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
        let parent = self
            .text(path, Instruction::Resolve(Revision::Head))
            .await?;
        let commit = self.commit(path, tree, &[&parent], KEPT).await?;
        self.update_branch(workspace, &commit, &parent).await
    }

    async fn forget(
        &self,
        repository: &str,
        prefix: &str,
    ) -> Result<(), GitError> {
        loop {
            let refs = self.references(repository, prefix, 500).await?;
            if refs.is_empty() {
                return Ok(());
            }
            self.delete_refs(repository, refs).await?;
        }
    }

    async fn delete_refs(
        &self,
        repository: &str,
        refs: Vec<String>,
    ) -> Result<(), GitError> {
        for name in refs {
            let _ = self
                .text(
                    repository,
                    Instruction::DeleteReference { reference: name },
                )
                .await?;
        }
        Ok(())
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
        let trees = self.uncommitted(workspace, false).await?;
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

fn workspace(owned: vcs::plan::Ownership) -> Workspace {
    Workspace {
        repository: Repository(owned.repository),
        original: WorkingCopy(owned.original),
        worktree: Worktree(owned.worktree),
        directory: SessionDirectory(owned.directory),
        branch: owned.branch,
    }
}

#[cfg(test)]
#[path = "plan_tests.rs"]
mod tests;
