//! The host's clone of the home (docs/home.md §3), its projects' checkouts
//! on this host, and the issues it writes.
//!
//! What the files say, which repositories are checkouts, where the clone
//! stands and what an issue change does to its file are the `home` core's
//! decisions; this task runs `git`, reads and writes the files and keeps
//! what the host stores of its home, and nothing else touches the clone.
//! In a checkout it found it writes only an issue's branch, `issue/<id>`,
//! before a session on the issue starts, and fetches the remote's; it
//! clones into its own data dir.
//!
//! An issue change is written and committed in the clone at once, and
//! kept in `<data>/home-edits.state` until the remote has it. When a push
//! is refused, or the host syncs with changes still kept, it resets the
//! clone to the remote and applies them again on top, so it never merges
//! its own commits.
//!
//! One task owns it; [`HomeClone`] handles talk to it over a channel. What
//! the home looks like now is on a watch channel, so a `Welcome` and a
//! subscription read it without waiting on a pull or a clone.

mod issue_file;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use home::checkout::{Probe, Root};
use home::flow::command::fresh;
use home::flow::graph::File;
use home::flow::snapshot::Snapshot;
use home::instructions::{Own, Sources};
use home::issue::{
    Commit, Context, Delivery, Edit, Listing, PendingEdits, Record,
    RecordEffect, RemoteProbe, SessionUpdate, Write,
};
use home::issue::{EditId, IssueChange, NewIssue};
use home::skills;
use home::sync::{self, Git, Sync};
use home::{
    Checkout, Cloning, Home, HomeCommand, HomeRefusal, HomeResult, HomeSync,
    IssueId, ProjectId,
};
use home::{Found, Stored};
use issue_file::{read_issue, write_issue};
use link::CommandResult;
use link::log::Completion;
use proto::{ClientName, HostPath, SessionRefusal, StartAt};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::{mpsc, oneshot, watch};

use crate::Stopped;
use crate::workspace::{blob, text};

/// How often the host pulls the home on its own.
const EVERY: Duration = Duration::from_secs(300);
/// A read pulls first unless the host tried within this many seconds.
const FRESH: u64 = 30;

/// Where the host keeps its home, and where it looks for checkouts.
#[derive(Debug, Clone)]
pub struct Homing {
    /// The host's name, which signs the notes it writes.
    pub host: String,
    pub data_dir: PathBuf,
    /// The home's remote from the command line; `None` keeps the one the
    /// host stored, if any.
    pub remote: Option<String>,
    /// The directories to look for checkouts in.
    pub roots: Vec<PathBuf>,
    /// The user's home, whose harness folders take the home's skills each
    /// time the clone moves; `None` installs none.
    pub user: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct HomeClone {
    inbox: mpsc::Sender<Msg>,
    shown: watch::Receiver<Option<Home>>,
    /// The clone, which a session's instructions are read from.
    clone: PathBuf,
}

pub use host::registry::Placement;

type Placed = Result<Placement, HomeRefusal>;

#[derive(Debug)]
enum IssueError {
    Refused(HomeRefusal),
    Retryable(String),
}

impl IssueError {
    fn local(refusal: HomeRefusal) -> Self {
        match refusal {
            HomeRefusal::IssueNotSaved { reason } => Self::Retryable(reason),
            other => other.into(),
        }
    }

    fn completion(self) -> Completion {
        match self {
            Self::Refused(refusal) => {
                CommandResult::Home(HomeResult::Refused(refusal)).into()
            }
            Self::Retryable(reason) => {
                Completion::Retryable(CommandResult::Home(HomeResult::Refused(
                    HomeRefusal::IssueNotSaved { reason },
                )))
            }
        }
    }
}

impl From<HomeRefusal> for IssueError {
    fn from(refusal: HomeRefusal) -> Self {
        Self::Refused(refusal)
    }
}

#[derive(Debug)]
enum Msg {
    Tick,
    Command {
        command: HomeCommand,
        by: ClientName,
        reply: oneshot::Sender<Completion>,
    },
    Place {
        at: StartAt,
        reply: oneshot::Sender<Placed>,
    },
    Ready {
        start: home::issue::Start,
        reply: oneshot::Sender<Result<(), SessionRefusal>>,
    },
    Started {
        issue: IssueId,
        client: ClientName,
        reply: oneshot::Sender<()>,
    },
    Finished {
        issue: IssueId,
        summary: Option<String>,
        client: ClientName,
        landed: u64,
        note_id: Option<EditId>,
        reply: oneshot::Sender<()>,
    },
}

impl HomeClone {
    /// Starts the task, which syncs the home at once and every few minutes.
    #[must_use]
    pub fn spawn(homing: Homing) -> Self {
        let (inbox, messages) = mpsc::channel(16);
        let (shown, watched) = watch::channel(None);
        let clone = clone_of(&homing.data_dir);
        let actor = Actor::new(homing, shown);
        actor.publish();
        tokio::spawn(actor.run(messages));
        Self {
            inbox,
            shown: watched,
            clone,
        }
    }

    /// What a session created in `created`, whose agent runs in
    /// `runs_in`, is told by a CLI that reads `file` by itself: the files
    /// as the clone and the directory it runs in have them now, read
    /// without waiting on the task once the checkout is verified.
    #[must_use]
    pub fn instructions(
        &self,
        created: &HostPath,
        runs_in: &HostPath,
        file: &str,
    ) -> Option<String> {
        let projects = match self.shown() {
            Some(home)
                if matches!(
                    home.sync,
                    HomeSync::Synced | HomeSync::Behind { .. }
                ) =>
            {
                home.projects
            }
            Some(_) => return None,
            None => Vec::new(),
        };
        let directory = Path::new(&runs_in.0);
        let own =
            home::instructions::project_of(&projects, &canonical(created)).map(
                |(project, _)| Own {
                    project,
                    rules: read(&directory.join(home::instructions::RULES)),
                    cli_reads_rules: directory.join(file).is_file(),
                },
            );
        home::instructions::compose(&Sources {
            home: self.clone.display().to_string(),
            shared: read(&self.clone.join(home::instructions::RULES)),
            own,
            projects: &projects,
        })
    }

    /// The home as it is now: what `Welcome` carries.
    #[must_use]
    pub fn shown(&self) -> Option<Home> {
        self.shown.borrow().clone()
    }

    /// Each change of the home, from now on.
    #[must_use]
    pub fn watch(&self) -> watch::Receiver<Option<Home>> {
        self.shown.clone()
    }

    /// Runs a home command once it has done what it asks.
    ///
    /// # Errors
    ///
    /// Fails if the task has stopped.
    pub(crate) async fn order(
        &self,
        command: HomeCommand,
        by: ClientName,
    ) -> Result<Completion, Stopped> {
        self.ask(|reply| Msg::Command { command, by, reply }).await
    }

    /// Where a session starts: a directory as it is, or a project's or an
    /// issue's checkout once the home is fresh.
    ///
    /// # Errors
    ///
    /// Fails if the task has stopped.
    pub async fn place(&self, at: StartAt) -> Result<Placed, Stopped> {
        match at {
            StartAt::Directory(directory) => Ok(Ok(Placement {
                directory,
                on: None,
            })),
            home => self.ask(|reply| Msg::Place { at: home, reply }).await,
        }
    }

    /// Readies the issue's branch in its checkout for a session to start
    /// from: the remote's, when it has one.
    ///
    /// # Errors
    ///
    /// Fails if the task has stopped.
    pub async fn ready(
        &self,
        start: home::issue::Start,
    ) -> Result<Result<(), SessionRefusal>, Stopped> {
        self.ask(|reply| Msg::Ready { start, reply }).await
    }

    /// A session on the issue started: work on it is in progress, if it
    /// wasn't already.
    ///
    /// # Errors
    ///
    /// Fails if the task has stopped.
    pub async fn started(
        &self,
        issue: IssueId,
        client: ClientName,
    ) -> Result<(), Stopped> {
        self.ask(|reply| Msg::Started {
            issue,
            client,
            reply,
        })
        .await
    }

    /// A session on the issue finished: its summary goes to the issue's
    /// notes, signed by `client`, and the issue is up for review when its
    /// branch gained commits.
    ///
    /// # Errors
    ///
    /// Fails if the task has stopped.
    pub async fn finished(
        &self,
        issue: IssueId,
        summary: Option<String>,
        client: ClientName,
        landed: u64,
        note_id: Option<EditId>,
    ) -> Result<(), Stopped> {
        self.ask(|reply| Msg::Finished {
            issue,
            summary,
            client,
            landed,
            note_id,
            reply,
        })
        .await
    }

    async fn ask<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<T>) -> Msg,
    ) -> Result<T, Stopped> {
        crate::ask(&self.inbox, build).await
    }
}

struct Actor {
    host: String,
    data_dir: PathBuf,
    /// The clone, `<data>/home-repo`.
    clone: PathBuf,
    /// Where "Clone here" clones a project, `<data>/projects`.
    clones: String,
    stored: PathBuf,
    remote: Option<String>,
    previous_remote: Option<String>,
    destination: Option<Destination>,
    roots: Vec<PathBuf>,
    sync: Sync,
    /// The clone as a scan or a replay last read it, which every issue
    /// change is decided against.
    home: Snapshot,
    found: BTreeMap<ProjectId, Vec<Checkout>>,
    cloning: BTreeMap<ProjectId, Cloning>,
    /// The issue changes the remote doesn't have yet, oldest first.
    edits: PendingEdits,
    edits_loaded: bool,
    edits_at: PathBuf,
    known: Vec<String>,
    shown: watch::Sender<Option<Home>>,
    user: Option<PathBuf>,
    /// The clone's commit whose skills were last installed.
    installed: Option<String>,
}

impl Actor {
    fn new(homing: Homing, shown: watch::Sender<Option<Home>>) -> Self {
        let Homing {
            host,
            data_dir,
            remote,
            roots,
            user,
        } = homing;
        let stored_at = data_dir.join("home.state");
        let stored: Stored = load(&stored_at);
        let edits_at = data_dir.join("home-edits.state");
        let clones = data_dir.join("projects");
        let _ = std::fs::create_dir_all(&clones);
        Self {
            clone: clone_of(&data_dir),
            clones: std::fs::canonicalize(&clones)
                .unwrap_or(clones)
                .to_string_lossy()
                .into_owned(),
            remote: stored.remote(remote),
            previous_remote: stored.remote.clone(),
            destination: None,
            known: stored.known(),
            host,
            data_dir,
            stored: stored_at,
            roots,
            sync: Sync::new(),
            home: Snapshot::empty(String::from(
                "the home has not been read yet",
            )),
            found: BTreeMap::new(),
            cloning: BTreeMap::new(),
            edits: PendingEdits::default(),
            edits_loaded: false,
            edits_at,
            shown,
            user,
            installed: None,
        }
    }

    async fn run(mut self, mut inbox: mpsc::Receiver<Msg>) {
        let mut ticks = tokio::time::interval(EVERY);
        while let Some(msg) = next(&mut inbox, &mut ticks).await {
            self.handle(msg).await;
        }
    }

    async fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Tick => self.refresh().await,
            Msg::Command { command, by, reply } => {
                let _ = reply.send(self.command(command, &by).await);
            }
            Msg::Place { at, reply } => {
                self.fresh().await;
                let shown = self.shown.borrow().clone();
                let _ = reply.send(host::registry::place(at, shown.as_ref()));
            }
            Msg::Ready { start, reply } => {
                let readied = ready_branch(&start).await.map_err(|reason| {
                    SessionRefusal::WorkspaceFailure { reason }
                });
                let _ = reply.send(readied);
            }
            Msg::Started {
                issue,
                client,
                reply,
            } => {
                self.session_update(
                    issue,
                    &client,
                    SessionUpdate::Started,
                    None,
                )
                .await;
                let _ = reply.send(());
            }
            Msg::Finished {
                issue,
                summary,
                client,
                landed,
                note_id,
                reply,
            } => {
                self.session_update(
                    issue,
                    &client,
                    SessionUpdate::Finished { summary, landed },
                    note_id,
                )
                .await;
                let _ = reply.send(());
            }
        }
    }

    async fn session_update(
        &mut self,
        issue: IssueId,
        client: &ClientName,
        mut update: SessionUpdate,
        note_id: Option<EditId>,
    ) {
        while let Some(change) =
            self.next_session_change(&mut update, issue).await
        {
            let id = SessionUpdate::edit_for(&change, note_id)
                .unwrap_or_else(|| EditId(ulid::Ulid::new().0));
            if let Err(error) = self.change(issue, change, client, id).await {
                let answer = error.completion().into_result();
                tracing::warn!(target: "home", ?answer, "a session's issue update was rejected");
            }
        }
    }

    async fn next_session_change(
        &mut self,
        update: &mut SessionUpdate,
        issue: IssueId,
    ) -> Option<IssueChange> {
        if *update == SessionUpdate::Done {
            return None;
        }
        self.refresh().await;
        update.next(&self.home, issue)
    }

    async fn command(
        &mut self,
        command: HomeCommand,
        by: &ClientName,
    ) -> Completion {
        match command {
            HomeCommand::Sync => home(self.pull_now().await),
            HomeCommand::Clone { project } => {
                home(self.clone_project(project).await)
            }
            HomeCommand::CreateIssue { edit, issue: new } => self
                .create(new, by, edit)
                .await
                .map_or_else(IssueError::completion, |(issue, warnings)| {
                    home(HomeResult::IssueWritten { issue, warnings })
                }),
            HomeCommand::ChangeIssue {
                edit,
                issue,
                change,
            } => self
                .change(issue, change, by, edit)
                .await
                .map_or_else(IssueError::completion, |warnings| {
                    home(HomeResult::IssueWritten { issue, warnings })
                }),
        }
    }

    async fn pull_now(&mut self) -> HomeResult {
        let configured = home::configured(self.remote.as_deref());
        if configured.is_ok() {
            self.refresh().await;
        }
        home::answer(configured)
    }

    /// Pulls before a read, unless the host tried a moment ago.
    async fn fresh(&mut self) {
        if self.sync.due(now(), FRESH) {
            self.refresh().await;
        }
    }

    /// Clones or pulls the home, or replays the host's changes onto it,
    /// then reads it again.
    async fn refresh(&mut self) {
        let Some(remote) = self.remote.clone() else {
            return;
        };
        let outcome = self.bring_up(&remote).await;
        self.sync =
            self.sync
                .clone()
                .after(outcome, self.destination.is_some(), now());
        if self.destination.is_some() {
            self.install().await;
            self.scan().await;
        } else {
            self.home =
                Snapshot::empty(String::from("the home has no checkout"));
            self.found.clear();
            self.publish();
        }
    }

    /// Installs the clone's skills for the user once per commit it moves
    /// to, as `home skills` does.
    async fn install(&mut self) {
        let moved = commit_of(&self.clone, "HEAD").await;
        let Some((user, head)) =
            skills::due(self.user.clone(), moved, self.installed.as_deref())
        else {
            return;
        };
        let clone = self.clone.clone();
        let done = tokio::task::spawn_blocking(move || {
            home_files::install(&clone, &user)
        })
        .await
        .map_err(|err| err.to_string())
        .and_then(core::convert::identity);
        match done {
            Ok(_) => self.installed = Some(head),
            Err(reason) => {
                tracing::warn!(%reason, "could not install the home's skills");
            }
        }
    }

    /// Runs what brings the clone up to date with the remote.
    async fn bring_up(&mut self, remote: &str) -> Result<(), String> {
        self.destination = None;
        let (destination, existing) = self.prepare_destination(remote).await?;
        self.destination = Some(destination.clone());
        self.load_edits()?;
        self.previous_remote.clone_from(&self.remote);
        match sync::next(existing, self.edits.count() != 0) {
            Git::Clone => self.replay_after_fetch().await,
            Git::Pull => {
                destination.fetch(&self.clone).await?;
                pull(&self.clone, &destination.tracking).await
            }
            Git::Replay => {
                if let Err(reason) = destination.fetch(&self.clone).await {
                    self.rewrite_all().await;
                    return Err(reason);
                }
                self.replay_after_fetch().await
            }
        }
    }

    async fn prepare_destination(
        &self,
        remote: &str,
    ) -> Result<(Destination, bool), String> {
        let address = remote_address(&self.data_dir, remote)?;
        self.check_pending_home(&address)?;
        let existing = cloned(&self.clone);
        if !existing {
            clone(&self.data_dir, &address, &self.clone).await?;
        }
        Destination::verified(&self.clone, address)
            .await
            .map(|destination| (destination, existing))
    }

    fn check_pending_home(&self, address: &str) -> Result<(), String> {
        let previous = self
            .previous_remote
            .as_deref()
            .filter(|_| self.edits_at.exists())
            .map(|remote| remote_address(&self.data_dir, remote))
            .transpose()?;
        if sync::queue_needs_check(previous.as_deref(), address)
            && !load_edits(&self.edits_at)?.is_empty()
        {
            return Err(String::from(
                "pending issue changes belong to the previous home",
            ));
        }
        Ok(())
    }

    async fn load_current_edits(&mut self) -> Result<(), IssueError> {
        home::configured(self.remote.as_deref())?;
        if self.destination.is_none() {
            self.fresh().await;
        }
        self.destination().map_err(IssueError::Retryable)?;
        self.load_edits().map_err(IssueError::Retryable)
    }

    fn destination(&self) -> Result<&Destination, String> {
        self.destination.as_ref().ok_or_else(|| {
            String::from("the configured home's checkout is unavailable")
        })
    }

    async fn replay_after_fetch(&mut self) -> Result<(), String> {
        self.deliver(Delivery::replay(self.edits.count() != 0))
            .await
    }

    async fn create(
        &mut self,
        new: NewIssue,
        client: &ClientName,
        edit: EditId,
    ) -> Result<(IssueId, Vec<String>), IssueError> {
        self.load_current_edits().await?;
        if let Some(kept) = self.kept_issue(edit) {
            return kept.map(|issue| (issue, Vec::new()));
        }
        self.fresh().await;
        self.destination().map_err(IssueError::Retryable)?;
        let issue = fresh(&self.home, now(), &draws()).ok_or_else(|| {
            IssueError::Retryable(String::from(
                "no issue id is free this second",
            ))
        })?;
        let request = self.issue_context(edit).open(issue, new, &client.0)?;
        let issue_dir = home::layout::issues(&request.project());
        if let Some(opened) = self.created(&issue_dir, edit).await? {
            return Ok((opened, Vec::new()));
        }
        let warnings = self.record(request).await?;
        Ok((issue, warnings))
    }

    async fn created(
        &self,
        path: &str,
        edit: EditId,
    ) -> Result<Option<IssueId>, IssueError> {
        if !cloned(&self.clone) {
            return Ok(None);
        }
        let pattern = Listing::creation_pattern(edit);
        let paths = text(
            &self.clone,
            &[
                "log",
                "--full-history",
                "-1",
                "--format=",
                "--name-only",
                "--no-renames",
                "--diff-filter=A",
                "-G",
                &pattern,
                "HEAD",
                "--",
                path,
            ],
        )
        .await
        .map_err(|err| IssueError::Retryable(err.to_string()))?;
        Ok(paths.lines().find_map(Listing::created_from_path))
    }

    async fn change(
        &mut self,
        issue: IssueId,
        change: IssueChange,
        client: &ClientName,
        edit: EditId,
    ) -> Result<Vec<String>, IssueError> {
        self.load_current_edits().await?;
        if let Some(kept) = self.kept_issue(edit) {
            return kept.map(|_| Vec::new());
        }
        self.fresh().await;
        self.destination().map_err(IssueError::Retryable)?;
        let request =
            self.issue_context(edit).edit(issue, change, &client.0)?;
        self.record(request).await
    }

    fn kept_issue(&self, id: EditId) -> Option<Result<IssueId, IssueError>> {
        let issue = self.edits.issue_for(id)?;
        Some(
            self.sync_edits()
                .map(|()| issue)
                .map_err(IssueError::Retryable),
        )
    }

    /// What an issue change is decided against.
    fn issue_context(&self, edit: EditId) -> Context<'_> {
        Context {
            remote: self.remote.as_deref(),
            home: &self.home,
            host: &self.host,
            at: now(),
            edit,
        }
    }

    /// Records the edit, written as the flow reads it back among the
    /// clone's other issues, and says what the flow warns of it.
    async fn record(
        &mut self,
        request: Record,
    ) -> Result<Vec<String>, IssueError> {
        self.home
            .flow()
            .map_err(|reason| HomeRefusal::IssueNotSaved {
                reason: String::from(reason),
            })?;
        let current = read_issue(&self.clone, &request.read().path)
            .map_err(IssueError::Retryable)?;
        let mut recorded = request
            .decide(current.as_deref(), &self.home, &self.edits)
            .map_err(IssueError::local)?;
        let warnings = recorded.warnings().to_vec();
        let mut outcome = Ok(());
        while let Some(effect) =
            recorded.next(outcome).map_err(IssueError::local)?
        {
            outcome = match effect {
                RecordEffect::Persist(edit) => self.persist(edit),
                RecordEffect::Commit(commit) => self.commit(commit).await,
                RecordEffect::Deliver { delivery, id } => {
                    self.edits.committed(id);
                    self.deliver(delivery).await
                }
                RecordEffect::Scan => {
                    self.scan().await;
                    Ok(())
                }
            };
        }
        Ok(warnings)
    }

    async fn deliver(&mut self, mut step: Delivery) -> Result<(), String> {
        let destination = self.destination()?.clone();
        loop {
            let outcome = match step {
                Delivery::Push { .. } => destination.push(&self.clone).await,
                Delivery::Fetch => destination.fetch(&self.clone).await,
                Delivery::Reset => {
                    reset(&self.clone, &destination.tracking).await
                }
                Delivery::Rewrite => {
                    self.rewrite_all().await;
                    Ok(())
                }
                Delivery::Landed => self.landed(),
                Delivery::Done => return Ok(()),
            };
            step = step.after(outcome.is_ok());
            if step == Delivery::Done {
                return outcome;
            }
        }
    }

    /// Applies every kept edit again to the clone as it is now.
    async fn rewrite_all(&mut self) {
        self.home = self.read_home();
        self.edits.reset();
        for index in 0..self.edits.count() {
            if let Some(read) = self.edits.replay_target(index) {
                let outcome = self.rewrite(index, &read.path).await.inspect_err(|reason| {
                    tracing::warn!(target: "home", issue = ?read.issue, %reason, "an issue change waits");
                });
                self.edits.replayed(index, outcome.is_ok());
            }
        }
    }

    async fn rewrite(
        &mut self,
        index: usize,
        path: &str,
    ) -> Result<(), String> {
        let changed =
            changed_at(&self.clone, &self.destination()?.tracking, path)
                .await?;
        let current = committed_issue(&self.clone, path).await;
        match self.edits.replay(
            &self.home,
            index,
            current
                .as_ref()
                .map(Option::as_deref)
                .map_err(String::clone),
            changed,
        ) {
            Write::Unchanged | Write::Dropped => Ok(()),
            Write::Waiting(reason) => Err(reason),
            Write::Commit(commit) => self.commit(commit).await,
        }
    }

    fn landed(&mut self) -> Result<(), String> {
        if let Some(remaining) = self.edits.after_push() {
            self.save_edits(remaining.stored())?;
            self.sync_edits()?;
            self.edits = remaining;
        }
        Ok(())
    }

    fn persist(&mut self, edit: Edit) -> Result<(), String> {
        self.save_edits(self.edits.with_recording(&edit))?;
        self.edits.installed(edit);
        self.sync_edits()
    }

    fn load_edits(&mut self) -> Result<(), String> {
        if self.edits_loaded {
            return Ok(());
        }
        let edits = load_edits(&self.edits_at)?;
        self.sync_edits()?;
        self.edits = PendingEdits::loaded(edits);
        self.edits_loaded = true;
        Ok(())
    }

    fn sync_edits(&self) -> Result<(), String> {
        crate::sync_dir(&self.data_dir)
            .map_err(|err| format!("could not sync the issue queue: {err}"))
    }

    fn save_edits<'a>(
        &self,
        edits: impl Iterator<Item = &'a Edit>,
    ) -> Result<(), String> {
        save(
            &self.edits_at,
            EDITS_VERSION,
            &edits.collect::<Vec<_>>(),
            "the unpushed issue changes",
        )
    }

    /// Writes and commits the issue's file, which the home is then read
    /// with.
    async fn commit(&mut self, commit: Commit) -> Result<(), String> {
        let target =
            write_issue(&self.clone, &commit.target.path, &commit.text)?;
        let result = commit_file(&self.clone, &commit).await;
        drop(target);
        self.home = self.home.with(File {
            project: commit.target.project,
            stem: commit.target.issue.to_string(),
            text: commit.text,
        });
        result
    }

    async fn clone_project(&mut self, project: ProjectId) -> HomeResult {
        self.fresh().await;
        let remote = home::remote_of(self.home.projects(), &project);
        if let Ok(from) = &remote {
            self.cloning.insert(project, Cloning::Running);
            self.publish();
            let target = home::checkout::clone_path(&self.clones, &project);
            let outcome = clone(&self.data_dir, from, Path::new(&target)).await;
            self.cloning.insert(project, home::cloned(outcome));
            self.scan().await;
        }
        home::answer(remote.map(drop))
    }

    /// Reads the home the clone holds and finds its projects' checkouts.
    async fn scan(&mut self) {
        self.home = self.read_home();
        self.home.problems().iter().for_each(|problem| {
            tracing::warn!(target: "home", %problem, "read the home without it");
        });
        let roots = self
            .roots
            .iter()
            .map(|path| home_files::root(path))
            .collect::<Vec<Root>>();
        let known = [
            self.known.clone(),
            home::checkout::clone_paths(&self.clones, self.home.projects()),
        ]
        .concat();
        let paths = home::checkout::candidates(&roots, &known);
        self.found = home::checkout::matched(
            self.home.projects(),
            &probes(paths).await,
            &self.clones,
        );
        let stored = Stored::kept(self.remote.clone(), &self.found);
        self.known = stored.known();
        let _ = save(&self.stored, &[], &stored, "the home's checkouts");
        self.publish();
    }

    /// The home the clone holds now.
    fn read_home(&self) -> Snapshot {
        home_files::read(&self.clone)
    }

    fn publish(&self) {
        self.shown
            .send_replace(self.remote.as_deref().map(|remote| {
                home::view(
                    remote,
                    &self.sync,
                    self.home.projects(),
                    Found {
                        checkouts: &self.found,
                        cloning: &self.cloning,
                        flow: self.home.flow().ok(),
                        issues: &self.home.issues(),
                        unpushed: u64::try_from(self.edits.count())
                            .unwrap_or(u64::MAX),
                    },
                )
            }));
    }
}

async fn next(
    inbox: &mut mpsc::Receiver<Msg>,
    ticks: &mut tokio::time::Interval,
) -> Option<Msg> {
    tokio::select! {
        msg = inbox.recv() => msg,
        _ = ticks.tick() => Some(Msg::Tick),
    }
}

fn cloned(clone: &Path) -> bool {
    clone.join(".git").exists()
}

/// `git clone`, run in the data dir.
async fn clone(
    data_dir: &Path,
    remote: &str,
    into: &Path,
) -> Result<(), String> {
    let address = remote_address(data_dir, remote)?;
    let target = into.to_string_lossy();
    text(
        data_dir,
        &[
            "clone", "--quiet", "--origin", "origin", "--", &address, &target,
        ],
    )
    .await
    .map(drop)
    .map_err(|err| remote_failure(remote, &err.to_string()))
}

fn remote_address(data_dir: &Path, remote: &str) -> Result<String, String> {
    if remote.trim().is_empty()
        || remote.trim_start().starts_with('-')
        || remote.chars().any(char::is_control)
    {
        return Err(String::from("invalid Git remote address"));
    }
    Ok(std::fs::canonicalize(data_dir.join(remote)).map_or_else(
        |_| remote.to_owned(),
        |local| local.to_string_lossy().into_owned(),
    ))
}

fn remote_failure(remote: &str, reason: &str) -> String {
    let display = home::checkout::display(remote);
    if display == remote {
        reason.to_owned()
    } else {
        reason.replace(remote, &display)
    }
}

#[derive(Clone)]
struct Destination {
    remote: String,
    branch: String,
    tracking: String,
}

impl Destination {
    async fn verified(clone: &Path, remote: String) -> Result<Self, String> {
        verify_checkout_path(clone).await?;
        let origin = text(clone, &["remote", "get-url", "origin"])
            .await
            .map_err(|_| {
                String::from("the existing home checkout has no origin")
            })?;
        let origin_address = remote_address(clone, &origin)?;
        if !sync::same_remote(&origin_address, &remote) {
            return Err(String::from(
                "the existing home checkout belongs to a different remote",
            ));
        }
        let tracking = text(
            clone,
            &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
        )
        .await
        .map_err(|_| {
            String::from("the home checkout has no origin default branch")
        })?;
        let head = text(clone, &["symbolic-ref", "--quiet", "HEAD"])
            .await
            .map_err(|_| String::from("the home checkout is detached"))?;
        let branch = sync::publishing_branch(&tracking, &head)?;
        Ok(Self {
            remote,
            branch,
            tracking,
        })
    }

    async fn fetch(&self, clone: &Path) -> Result<(), String> {
        let refspec = format!("+{}:{}", self.branch, self.tracking);
        run(
            clone,
            &[
                "fetch",
                "--quiet",
                "--no-tags",
                "--refmap=",
                "--",
                &self.remote,
                &refspec,
            ],
        )
        .await
        .map_err(|reason| remote_failure(&self.remote, &reason))
    }

    async fn push(&self, clone: &Path) -> Result<(), String> {
        let refspec = format!("{}:{}", self.branch, self.branch);
        run(
            clone,
            &[
                "push",
                "--quiet",
                "--no-follow-tags",
                "--",
                &self.remote,
                &refspec,
            ],
        )
        .await
        .map_err(|reason| remote_failure(&self.remote, &reason))
    }
}

async fn verify_checkout_path(clone: &Path) -> Result<(), String> {
    let root =
        std::fs::symlink_metadata(clone).map_err(|err| err.to_string())?;
    if !root.is_dir() {
        return Err(String::from("the home checkout is not a directory"));
    }
    let top = text(clone, &["rev-parse", "--show-toplevel"])
        .await
        .map_err(|err| err.to_string())?;
    let canonical = |path: &Path| {
        std::fs::canonicalize(path).map_err(|err| err.to_string())
    };
    if canonical(clone)? != canonical(Path::new(&top))? {
        return Err(String::from(
            "the home checkout is not at its managed path",
        ));
    }
    Ok(())
}

async fn pull(clone: &Path, tracking: &str) -> Result<(), String> {
    let pulled = run(clone, &["rebase", "--quiet", tracking]).await;
    if pulled.is_err() {
        let _ = text(clone, &["rebase", "--abort"]).await;
    }
    pulled
}

/// Makes the checkout's issue branch what `home::issue::Branch` says. The
/// remote is `origin`, which is how the home matched the checkout.
async fn ready_branch(start: &home::issue::Start) -> Result<(), String> {
    ready_issue_branch(Path::new(&start.checkout.0), &start.branch).await
}

/// Makes the checkout's issue branch `branch`, a full ref, what
/// `home::issue::Branch` says: the remote's, when the checkout's has
/// nothing the remote's lacks.
///
/// # Errors
///
/// When the remote can't be read for a new branch, or both have moved on.
pub async fn ready_issue_branch(
    checkout: &Path,
    branch: &str,
) -> Result<(), String> {
    let facts = branch_facts(checkout, branch).await?;
    match facts.next() {
        home::issue::Move::Keep => Ok(()),
        home::issue::Move::Refuse(reason) => Err(reason),
        home::issue::Move::Set { commit, old } => {
            let listed =
                text(checkout, &["worktree", "list", "--porcelain", "-z"])
                    .await
                    .map_err(|error| error.to_string())?;
            if let Some(path) = host::workspace::checked_out(&listed, branch) {
                return Err(format!(
                    "the issue branch is checked out at {path}; switch that worktree to another branch before synchronizing"
                ));
            }
            let was = old.unwrap_or_default();
            run(checkout, &["update-ref", branch, &commit, &was]).await
        }
    }
}

async fn branch_facts(
    checkout: &Path,
    branch: &str,
) -> Result<home::issue::Branch, String> {
    let local = commit_of(checkout, branch).await;
    let remote = remote_branch(checkout, branch).await;
    let at = match &remote {
        home::issue::Remote::At(commit) => Some(commit.as_str()),
        home::issue::Remote::Absent
        | home::issue::Remote::Unreachable(_)
        | home::issue::Remote::PresentUnfetched(_) => None,
    };
    let behind = ancestor(checkout, local.as_deref(), at).await;
    let ahead = ancestor(checkout, at, local.as_deref()).await;
    Ok(home::issue::Branch {
        base: base(checkout).await?,
        local,
        remote,
        behind,
        ahead,
    })
}

async fn remote_branch(checkout: &Path, branch: &str) -> home::issue::Remote {
    let mut probe = RemoteProbe::for_branch(branch);
    loop {
        let outcome = match &probe {
            RemoteProbe::Origin { .. } => {
                text(checkout, &["remote", "get-url", "origin"]).await
            }
            RemoteProbe::Fetch { refspec, .. } => {
                text(checkout, &["fetch", "--quiet", "origin", refspec]).await
            }
            RemoteProbe::Tracking(tracking) => {
                let wanted = format!("{tracking}^{{commit}}");
                text(checkout, &["rev-parse", "--verify", "--quiet", &wanted])
                    .await
            }
            RemoteProbe::Reachability { branch, .. } => {
                text(
                    checkout,
                    &["ls-remote", "--quiet", "--refs", "origin", branch],
                )
                .await
            }
            RemoteProbe::Observed(remote) => return remote.clone(),
        };
        probe = probe.after(outcome.map_err(|err| err.to_string()));
    }
}

/// Where a new issue branch starts: the remote's default branch as the
/// checkout knows it, else its `HEAD`.
async fn base(checkout: &Path) -> Result<String, String> {
    let remote = commit_of(checkout, "refs/remotes/origin/HEAD").await;
    let head = commit_of(checkout, "HEAD").await;
    remote
        .or(head)
        .ok_or_else(|| String::from("the checkout has no commit"))
}

async fn commit_of(repository: &Path, reference: &str) -> Option<String> {
    let wanted = format!("{reference}^{{commit}}");
    text(repository, &["rev-parse", "--verify", "--quiet", &wanted])
        .await
        .ok()
}

/// Whether `older` is an ancestor of `newer`, when the checkout has both.
async fn ancestor(
    repository: &Path,
    older: Option<&str>,
    newer: Option<&str>,
) -> bool {
    let Some((first, then)) = older.zip(newer) else {
        return false;
    };
    run(repository, &["merge-base", "--is-ancestor", first, then])
        .await
        .is_ok()
}

async fn reset(clone: &Path, tracking: &str) -> Result<(), String> {
    run(clone, &["reset", "--quiet", "--hard", tracking]).await
}

async fn run(clone: &Path, args: &[&str]) -> Result<(), String> {
    text(clone, args)
        .await
        .map(drop)
        .map_err(|err| err.to_string())
}

async fn changed_at(
    clone: &Path,
    tracking: &str,
    relative: &str,
) -> Result<Option<u64>, String> {
    let seconds = text(
        clone,
        &[
            "log",
            "--full-history",
            "-1",
            "--format=%ct",
            tracking,
            "--",
            relative,
        ],
    )
    .await
    .map_err(|err| err.to_string())?;
    if seconds.is_empty() {
        return Ok(None);
    }
    seconds
        .trim()
        .parse()
        .map(Some)
        .map_err(|err: std::num::ParseIntError| err.to_string())
}

/// Commits the file at `relative` alone, as Conductor.
async fn commit_file(clone: &Path, commit: &Commit) -> Result<(), String> {
    let relative = &commit.target.path;
    text(clone, &["add", "--", relative])
        .await
        .map_err(|err| err.to_string())?;
    text(
        clone,
        &[
            "-c",
            &format!("user.name={}", commit.author),
            "-c",
            &format!("user.email={}", commit.email),
            "commit",
            "--quiet",
            "-m",
            &commit.message,
            "--",
            relative,
        ],
    )
    .await
    .map(drop)
    .map_err(|err| err.to_string())
}

async fn probes(paths: Vec<String>) -> Vec<Probe> {
    let mut found = Vec::new();
    for path in paths {
        found.extend(probe(&path).await);
    }
    found
}

/// What Git says of the repository at `path`, by its canonical path, each
/// of [`home::checkout::ASKED`] run in turn with the daemon's time limit.
async fn probe(path: &str) -> Option<Probe> {
    let canonical = std::fs::canonicalize(path).ok()?;
    let shown = canonical.to_str()?.to_owned();
    let [asked_top, asked_roots, asked_remote] = home::checkout::ASKED;
    let top = text(&canonical, asked_top).await.ok();
    let roots = text(&canonical, asked_roots).await.ok();
    let remote = text(&canonical, asked_remote).await.ok();
    home::checkout::probed(
        &shown,
        top.as_deref(),
        roots.as_deref(),
        remote.as_deref(),
    )
}

/// The host's clone of the home, `<data>/home-repo`.
fn clone_of(data_dir: &Path) -> PathBuf {
    data_dir.join("home-repo")
}

/// `path` as checkouts are found, through any symlink.
fn canonical(path: &HostPath) -> String {
    std::fs::canonicalize(&path.0)
        .ok()
        .and_then(|resolved| resolved.to_str().map(str::to_owned))
        .unwrap_or_else(|| path.0.clone())
}

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// The store's encoding. A store in any other is set aside, never read.
const EDITS_VERSION: &[u8] = b"\x80\x00\x06";

/// The unpushed issue changes the host kept. A store that no longer reads
/// is moved aside, so its changes are not lost unseen.
fn load_edits(path: &Path) -> Result<Vec<Edit>, String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(err) => {
            return Err(format!("could not read the issue queue: {err}"));
        }
    };
    decode_edits(&bytes).or_else(|err| {
        let aside = path.with_extension("unreadable");
        tracing::warn!(target: "home", %err, aside = %aside.display(), "could not decode the unpushed issue changes");
        std::fs::rename(path, aside)
            .map(|()| Vec::new())
            .map_err(|err| format!("could not move the unreadable issue queue: {err}"))
    })
}

fn decode_edits(bytes: &[u8]) -> Result<Vec<Edit>, postcard::Error> {
    let (edits, rest) = postcard::take_from_bytes(
        bytes
            .strip_prefix(EDITS_VERSION)
            .ok_or(postcard::Error::DeserializeBadEncoding)?,
    )?;
    if !rest.is_empty() {
        return Err(postcard::Error::DeserializeBadEncoding);
    }
    Ok(edits)
}

/// What the host stored at `path`, or nothing if it can't be read.
fn load<T: DeserializeOwned + Default>(path: &Path) -> T {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| postcard::from_bytes(&bytes).ok())
        .unwrap_or_default()
}

/// Writes the store beside its file, then renames it over, so a crash
/// leaves either.
fn save<T: Serialize + ?Sized>(
    path: &Path,
    prefix: &[u8],
    stored: &T,
    what: &str,
) -> Result<(), String> {
    let written = postcard::to_extend(stored, prefix.to_vec())
        .map_err(anyhow::Error::from)
        .and_then(|bytes| replace(path, &bytes));
    if let Err(err) = &written {
        tracing::warn!(target: "home", %err, "could not store {what}");
    }
    written.map_err(|err| format!("could not store {what}: {err}"))
}

fn replace(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;

    let next = path.with_extension("new");
    let mut file = std::fs::File::create(&next)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&next, path)?;
    Ok(())
}

async fn committed_issue(
    clone: &Path,
    path: &str,
) -> Result<Option<String>, String> {
    committed_blob(clone, path)
        .await?
        .map(String::from_utf8)
        .transpose()
        .map_err(|err| err.to_string())
}

async fn committed_blob(
    clone: &Path,
    path: &str,
) -> Result<Option<Vec<u8>>, String> {
    committed_blob_at(clone, "HEAD", path).await
}

async fn committed_blob_at(
    clone: &Path,
    revision: &str,
    path: &str,
) -> Result<Option<Vec<u8>>, String> {
    let tracked =
        text(clone, &["ls-tree", "--name-only", revision, "--", path])
            .await
            .map_err(|err| err.to_string())?;
    if tracked.is_empty() {
        return Ok(None);
    }
    blob(clone, &format!("{revision}:{path}"))
        .await
        .map(Some)
        .map_err(|err| err.to_string())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// The randomness a new issue's id is minted with.
fn draws() -> home::Draws {
    core::array::from_fn(|_| getrandom::u64().unwrap_or_default())
}

fn home(result: HomeResult) -> Completion {
    CommandResult::Home(result).into()
}

#[cfg(test)]
mod tests {
    use super::{Actor, Destination, Homing, clone, remote_branch, run, text};
    use home::issue::Remote;
    use std::path::{Path, PathBuf};
    use tokio::sync::watch;

    type Outcome = Result<(), Box<dyn std::error::Error>>;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> std::io::Result<Self> {
            let path = std::env::temp_dir()
                .join(format!("conductor-home-test-{}", ulid::Ulid::new()));
            std::fs::create_dir(&path)?;
            Ok(Self(path))
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    async fn seed(path: &Path) -> Outcome {
        std::fs::create_dir(path)?;
        run(path, &["init", "-q", "-b", "main"]).await?;
        add_commit(path, "initial").await
    }

    async fn add_commit(path: &Path, message: &str) -> Outcome {
        run(
            path,
            &[
                "-c",
                "user.name=Tests",
                "-c",
                "user.email=tests@localhost",
                "commit",
                "-qm",
                message,
                "--allow-empty",
            ],
        )
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn clone_rejects_invalid_remotes_and_can_clone_a_relative_local_path()
    -> Outcome {
        let scratch = Scratch::new()?;
        let root = &scratch.0;
        let dest = root.join("destination");
        for remote in [
            "",
            "   ",
            "--not-a-remote",
            " --not-a-remote",
            "remote\npath",
        ] {
            assert_eq!(
                clone(root, remote, &dest).await,
                Err(String::from("invalid Git remote address"))
            );
            assert!(!dest.exists());
        }
        seed(&root.join("local repo")).await?;
        clone(root, "local repo", &dest).await?;
        assert_eq!(
            text(&dest, &["rev-parse", "HEAD"]).await?,
            text(&root.join("local repo"), &["rev-parse", "HEAD"]).await?,
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_pending_home_commit_stays_with_its_verified_destination()
    -> Outcome {
        let scratch = Scratch::new()?;
        let root = &scratch.0;
        let source = root.join("source");
        seed(&source).await?;
        let origin = root.join("origin.git");
        let other = root.join("other.git");
        let source_url = source.to_str().ok_or("invalid source path")?;
        for name in ["origin.git", "other.git"] {
            run(root, &["clone", "--bare", "--", source_url, name]).await?;
        }
        let original_url = origin.to_str().ok_or("invalid origin path")?;
        let other_url = other.to_str().ok_or("invalid other path")?;
        let managed = root.join("home-repo");
        clone(root, original_url, &managed).await?;
        run(&managed, &["remote", "add", "elsewhere", other_url]).await?;
        run(&managed, &["config", "remote.pushDefault", "elsewhere"]).await?;
        add_commit(&managed, "pending").await?;
        let pending = text(&managed, &["rev-parse", "HEAD"]).await?;
        let previous = text(&other, &["rev-parse", "HEAD"]).await?;
        let (shown, _) = watch::channel(None);
        let mut actor = Actor::new(
            Homing {
                host: String::from("Tests"),
                data_dir: root.clone(),
                remote: Some(other_url.to_owned()),
                roots: Vec::new(),
                user: None,
            },
            shown,
        );
        assert!(actor.bring_up(other_url).await.is_err());
        assert_eq!(text(&managed, &["rev-parse", "HEAD"]).await?, pending);
        assert_eq!(text(&origin, &["rev-parse", "HEAD"]).await?, previous);
        let intended =
            Destination::verified(&managed, original_url.to_owned()).await?;
        intended.push(&managed).await?;
        assert_eq!(text(&origin, &["rev-parse", "HEAD"]).await?, pending);
        assert_eq!(text(&other, &["rev-parse", "HEAD"]).await?, previous);
        Ok(())
    }

    #[tokio::test]
    async fn a_present_remote_branch_with_a_failed_tracking_update_blocks_a_start()
    -> Outcome {
        let scratch = Scratch::new()?;
        let root = &scratch.0;
        let source = root.join("source");
        let checkout = root.join("checkout");
        seed(&source).await?;
        run(&source, &["branch", "issue/task"]).await?;
        clone(
            root,
            source.to_str().ok_or("invalid source path")?,
            &checkout,
        )
        .await?;
        run(&source, &["branch", "-D", "issue/task"]).await?;
        add_commit(&source, "new issue tip").await?;
        run(&source, &["branch", "issue/task"]).await?;
        let lock = checkout.join(".git/refs/remotes/origin/issue/task.lock");
        std::fs::create_dir_all(lock.parent().ok_or("invalid ref lock path")?)?;
        std::fs::File::create(lock)?;
        assert!(matches!(
            remote_branch(&checkout, "refs/heads/issue/task").await,
            Remote::PresentUnfetched(_)
        ));
        assert_eq!(
            remote_branch(&checkout, "refs/heads/issue/absent").await,
            Remote::Absent
        );
        Ok(())
    }

    #[test]
    fn nested_issue_reads_and_atomic_replacements_use_the_checkout() -> Outcome
    {
        let scratch = Scratch::new()?;
        let checkout = scratch.0.join("checkout");
        std::fs::create_dir(&checkout)?;
        let path = "projects/alpha/issues/issue.md";
        assert_eq!(super::read_issue(&checkout, path)?, None);
        for contents in ["original", "replayed"] {
            let written = super::write_issue(&checkout, path, contents)?;
            assert_eq!(
                super::read_issue(&checkout, path)?.as_deref(),
                Some(contents)
            );
            drop(written);
        }
        assert_eq!(
            std::fs::read_dir(checkout.join("projects/alpha/issues"))?.count(),
            1
        );
        assert!(
            super::write_issue(&checkout, "new/../outside/issue.md", "edited")
                .is_err()
        );
        assert!(!checkout.join("new").try_exists()?);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn an_issue_path_rejects_linked_components_before_creating_directories()
    -> Outcome {
        let scratch = Scratch::new()?;
        let checkout = scratch.0.join("checkout");
        let outside = scratch.0.join("outside");
        std::fs::create_dir(&checkout)?;
        std::fs::create_dir(&outside)?;
        let sentinel = outside.join("sentinel");
        std::fs::write(&sentinel, b"unchanged")?;
        std::os::unix::fs::symlink(&outside, checkout.join("projects"))?;
        assert!(
            super::write_issue(&checkout, "projects/new/issue.md", "edited")
                .is_err()
        );
        assert!(!outside.join("new").try_exists()?);
        assert!(super::read_issue(&checkout, "projects/sentinel").is_err());
        let linked_checkout = scratch.0.join("linked-checkout");
        std::os::unix::fs::symlink(&outside, &linked_checkout)?;
        assert!(
            super::write_issue(&linked_checkout, "sentinel", "edited").is_err()
        );
        std::fs::remove_file(checkout.join("projects"))?;
        std::fs::create_dir(checkout.join("projects"))?;
        std::os::unix::fs::symlink(
            &sentinel,
            checkout.join("projects/issue.md"),
        )?;
        assert!(
            super::write_issue(&checkout, "projects/issue.md", "edited")
                .is_err()
        );
        assert!(super::read_issue(&checkout, "projects/issue.md").is_err());
        assert_eq!(std::fs::read(sentinel)?, b"unchanged");
        Ok(())
    }
}
