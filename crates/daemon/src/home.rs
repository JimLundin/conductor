//! The host's clone of the home (docs/home.md §3), its projects' checkouts
//! on this host, and the issues it writes.
//!
//! What the files say, which repositories are checkouts, where the clone
//! stands and what an issue change does to its file are the `home` core's
//! decisions; this task runs `git`, reads and writes the files and keeps
//! what the host stores of its home, and nothing else touches the clone.
//! It never writes in a checkout it found: it only reads them, and clones
//! into its own data dir.
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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use home::checkout::{Probe, Root};
use home::issue::{Applied, Change, Edit};
use home::project::Project;
use home::sync::{Git, Sync};
use home::{Found, Stored};
use host::serve::HomeOrder;
use proto::{
    Checkout, ClientName, Cloning, CommandResult, Home, HostPath, Issue,
    IssueChange, IssueFields, IssueId, ProjectId, Rejection, StartAt,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::{mpsc, oneshot, watch};

use crate::Stopped;
use crate::workspace::text;

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
}

#[derive(Debug, Clone)]
pub struct HomeClone {
    inbox: mpsc::Sender<Msg>,
    shown: watch::Receiver<Option<Home>>,
}

type Placed = Result<HostPath, Rejection>;

#[derive(Debug)]
enum Msg {
    Tick,
    Order {
        order: HomeOrder,
        reply: oneshot::Sender<CommandResult>,
    },
    Place {
        project: ProjectId,
        reply: oneshot::Sender<Placed>,
    },
}

impl HomeClone {
    /// Starts the task. It reads what it has at once, then clones or pulls
    /// the home, and pulls it again every few minutes.
    #[must_use]
    pub fn spawn(homing: Homing) -> Self {
        let (inbox, messages) = mpsc::channel(16);
        let (shown, watched) = watch::channel(None);
        tokio::spawn(Actor::new(homing, shown).run(messages));
        Self {
            inbox,
            shown: watched,
        }
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
    pub async fn order(
        &self,
        order: HomeOrder,
    ) -> Result<CommandResult, Stopped> {
        self.ask(|reply| Msg::Order { order, reply }).await
    }

    /// Where a session starts: a directory as it is, or a project's
    /// checkout once the home is fresh.
    ///
    /// # Errors
    ///
    /// Fails if the task has stopped.
    pub async fn place(&self, at: StartAt) -> Result<Placed, Stopped> {
        match at {
            StartAt::Directory(directory) => Ok(Ok(directory)),
            StartAt::Project(project) => {
                self.ask(|reply| Msg::Place { project, reply }).await
            }
        }
    }

    async fn ask<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<T>) -> Msg,
    ) -> Result<T, Stopped> {
        let (reply, answer) = oneshot::channel();
        self.inbox
            .send(build(reply))
            .await
            .map_err(|_| Stopped {})?;
        answer.await.map_err(|_| Stopped {})
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
    roots: Vec<PathBuf>,
    sync: Sync,
    projects: Vec<Project>,
    found: BTreeMap<ProjectId, Vec<Checkout>>,
    cloning: BTreeMap<ProjectId, Cloning>,
    issues: BTreeMap<ProjectId, Vec<Issue>>,
    /// The issue changes the remote doesn't have yet, oldest first.
    edits: Vec<Edit>,
    edits_at: PathBuf,
    known: Vec<String>,
    shown: watch::Sender<Option<Home>>,
}

impl Actor {
    fn new(homing: Homing, shown: watch::Sender<Option<Home>>) -> Self {
        let Homing {
            host,
            data_dir,
            remote,
            roots,
        } = homing;
        let stored_at = data_dir.join("home.state");
        let stored: Stored = load(&stored_at);
        let edits_at = data_dir.join("home-edits.state");
        let clones = data_dir.join("projects");
        let _ = std::fs::create_dir_all(&clones);
        Self {
            clone: data_dir.join("home-repo"),
            clones: std::fs::canonicalize(&clones)
                .unwrap_or(clones)
                .to_string_lossy()
                .into_owned(),
            remote: stored.remote(remote),
            known: stored.known(),
            host,
            data_dir,
            stored: stored_at,
            roots,
            sync: Sync::new(),
            projects: Vec::new(),
            found: BTreeMap::new(),
            cloning: BTreeMap::new(),
            issues: BTreeMap::new(),
            edits: load(&edits_at),
            edits_at,
            shown,
        }
    }

    async fn run(mut self, mut inbox: mpsc::Receiver<Msg>) {
        self.scan().await;
        let mut ticks = tokio::time::interval(EVERY);
        while let Some(msg) = next(&mut inbox, &mut ticks).await {
            self.handle(msg).await;
        }
    }

    async fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Tick => self.refresh().await,
            Msg::Order {
                order: HomeOrder::Sync,
                reply,
            } => {
                let _ = reply.send(self.pull_now().await);
            }
            Msg::Order {
                order: HomeOrder::Clone(project),
                reply,
            } => {
                let _ = reply.send(self.clone_project(project).await);
            }
            Msg::Order {
                order: HomeOrder::Create(project, fields),
                reply,
            } => {
                let _ = reply.send(self.create(project, fields).await);
            }
            Msg::Order {
                order: HomeOrder::Change(issue, change, client),
                reply,
            } => {
                let _ = reply.send(self.change(issue, change, &client).await);
            }
            Msg::Place { project, reply } => {
                self.fresh().await;
                let shown = self.shown.borrow().clone();
                let _ = reply.send(home::place(
                    StartAt::Project(project),
                    shown.as_ref(),
                ));
            }
        }
    }

    async fn pull_now(&mut self) -> CommandResult {
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
            self.sync.clone().after(outcome, cloned(&self.clone), now());
        self.scan().await;
    }

    /// Runs what brings the clone up to date with the remote.
    async fn bring_up(&mut self, remote: &str) -> Result<(), String> {
        match home::sync::next(cloned(&self.clone), !self.edits.is_empty()) {
            Git::Clone => {
                let cloned = clone(&self.data_dir, remote, &self.clone).await;
                self.replay_after(cloned).await
            }
            Git::Pull => pull(&self.clone).await,
            Git::Replay => {
                let fetched = fetch(&self.clone).await;
                self.replay_after(fetched).await
            }
        }
    }

    /// Replays the kept changes once the remote has been read.
    async fn replay_after(
        &mut self,
        read: Result<(), String>,
    ) -> Result<(), String> {
        if read.is_ok() {
            self.replay().await;
        }
        read
    }

    async fn create(
        &mut self,
        project: ProjectId,
        wanted: IssueFields,
    ) -> CommandResult {
        self.fresh().await;
        let projects = self
            .projects
            .iter()
            .map(|known| known.id.clone())
            .collect::<Vec<ProjectId>>();
        match home::issue::opening(
            self.remote.as_deref(),
            &projects,
            &project,
            &wanted,
        ) {
            Err(rejection) => CommandResult::Rejected(rejection),
            Ok(fields) => {
                let issue = IssueId(ulid::Ulid::new().0);
                let edit = Edit {
                    project,
                    issue,
                    at: now(),
                    by: self.host.clone(),
                    change: Change::Create(fields),
                };
                match self.record(edit).await {
                    Ok(()) => CommandResult::IssueCreated { issue },
                    Err(rejection) => CommandResult::Rejected(rejection),
                }
            }
        }
    }

    async fn change(
        &mut self,
        issue: IssueId,
        change: IssueChange,
        client: &ClientName,
    ) -> CommandResult {
        self.fresh().await;
        let changing = home::issue::changing(
            self.remote.as_deref(),
            &self.issues,
            issue,
            change,
        );
        let recorded = match changing {
            Err(rejection) => Err(rejection),
            Ok((project, change)) => {
                let edit = Edit {
                    project,
                    issue,
                    at: now(),
                    by: home::issue::signer(&self.host, &client.0),
                    change: Change::Issue(change),
                };
                self.record(edit).await
            }
        };
        home::answer(recorded)
    }

    /// Writes and commits the change in the clone, keeps it until the
    /// remote has it, and pushes.
    async fn record(&mut self, edit: Edit) -> Result<(), Rejection> {
        match self.write(&edit).await {
            Ok(true) => {}
            Ok(false) => return Err(Rejection::UnknownIssue),
            Err(reason) => return Err(Rejection::IssueNotSaved { reason }),
        }
        self.edits.push(edit);
        save(&self.edits_at, &self.edits, "the unpushed issue changes");
        self.land().await;
        self.scan().await;
        Ok(())
    }

    /// Pushes the kept changes; when the remote refuses them, replays them
    /// on top of it and pushes again.
    async fn land(&mut self) {
        if push(&self.clone).await.is_ok() {
            self.landed();
        } else if fetch(&self.clone).await.is_ok() {
            self.replay().await;
        }
    }

    /// Resets the clone to the remote, applies the kept changes on top,
    /// each its own commit, and pushes them. A change whose issue is gone
    /// is dropped; one that can't be written is kept for the next try.
    async fn replay(&mut self) {
        if self.edits.is_empty() || !self.reset().await {
            return;
        }
        self.rewrite_all().await;
        save(&self.edits_at, &self.edits, "the unpushed issue changes");
        self.push_kept().await;
    }

    async fn rewrite_all(&mut self) {
        let mut kept = Vec::new();
        for edit in core::mem::take(&mut self.edits) {
            kept.extend(self.rewrite(edit).await);
        }
        self.edits = kept;
    }

    /// Moves the clone to the remote, whether it could.
    async fn reset(&self) -> bool {
        match reset(&self.clone).await {
            Ok(()) => true,
            Err(reason) => {
                tracing::warn!(target: "home", %reason, "could not reset the home to replay its changes");
                false
            }
        }
    }

    /// Writes a kept change again, and keeps it unless its issue is gone.
    async fn rewrite(&self, edit: Edit) -> Option<Edit> {
        match self.write(&edit).await {
            Ok(true) => Some(edit),
            Ok(false) => {
                tracing::warn!(target: "home", issue = %home::issue::name(edit.issue), "dropped a change to an issue the home no longer has");
                None
            }
            Err(reason) => {
                tracing::warn!(target: "home", %reason, "could not replay an issue change");
                Some(edit)
            }
        }
    }

    async fn push_kept(&mut self) {
        if push(&self.clone).await.is_ok() {
            self.landed();
        }
    }

    fn landed(&mut self) {
        self.edits.clear();
        save(&self.edits_at, &self.edits, "the unpushed issue changes");
    }

    /// Applies the change to the issue's file in the clone and commits it;
    /// `false` when its issue is gone.
    ///
    /// # Errors
    ///
    /// When it can't be written or committed.
    async fn write(&self, edit: &Edit) -> Result<bool, String> {
        let relative = home::issue::file(&edit.project, edit.issue);
        let path = self.clone.join(&relative);
        let current = std::fs::read_to_string(&path).ok();
        match home::issue::apply(current.as_deref(), edit) {
            Applied::Unchanged => Ok(true),
            Applied::Gone => Ok(false),
            Applied::Write(text) => {
                if let Some(folder) = path.parent() {
                    std::fs::create_dir_all(folder)
                        .map_err(|err| err.to_string())?;
                }
                std::fs::write(&path, text).map_err(|err| err.to_string())?;
                commit(&self.clone, &relative, &home::issue::message(edit))
                    .await
                    .map(|()| true)
            }
        }
    }

    async fn clone_project(&mut self, project: ProjectId) -> CommandResult {
        self.fresh().await;
        let remote = home::remote_of(&self.projects, &project);
        if let Ok(from) = &remote {
            self.cloning.insert(project.clone(), Cloning::Running);
            self.publish();
            let target = home::checkout::clone_path(&self.clones, &project);
            let outcome = clone(&self.data_dir, from, Path::new(&target)).await;
            self.cloning.insert(project, home::cloned(outcome));
            self.scan().await;
        }
        home::answer(remote.map(drop))
    }

    /// Reads the projects the clone lists and finds their checkouts.
    async fn scan(&mut self) {
        let listing = home::project::listed(folders(&self.clone));
        for problem in &listing.problems {
            tracing::warn!(target: "home", %problem, "skipped a project");
        }
        self.projects = listing.projects;
        self.issues = self
            .projects
            .iter()
            .map(|project| {
                let folder = home::issue::folder(&project.id);
                let (issues, problems) = home::issue::listed(
                    &project.id,
                    issue_files(&self.clone.join(folder)),
                );
                for problem in &problems {
                    tracing::warn!(target: "home", %problem, "skipped an issue");
                }
                (project.id.clone(), issues)
            })
            .collect();
        let roots = self
            .roots
            .iter()
            .map(|path| root(path))
            .collect::<Vec<Root>>();
        let paths = home::checkout::candidates(
            &roots,
            &self.projects,
            &self.known,
            &self.clones,
        );
        self.found = home::checkout::matched(
            &self.projects,
            &probes(paths).await,
            &self.clones,
        );
        let stored = Stored::kept(self.remote.clone(), &self.found);
        self.known = stored.known();
        save(&self.stored, &stored, "the home's checkouts");
        self.publish();
    }

    fn publish(&self) {
        self.shown
            .send_replace(self.remote.as_deref().map(|remote| {
                home::view(
                    remote,
                    &self.sync,
                    &self.projects,
                    Found {
                        checkouts: &self.found,
                        cloning: &self.cloning,
                        issues: &self.issues,
                        unpushed: u64::try_from(self.edits.len())
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
    let target = into.to_string_lossy();
    text(data_dir, &["clone", "--quiet", remote, &target])
        .await
        .map(drop)
        .map_err(|err| err.to_string())
}

/// `git pull --rebase`, which fast-forwards when it can. A rebase that
/// stops is aborted, so the clone stays as it was.
async fn pull(clone: &Path) -> Result<(), String> {
    let pulled = text(clone, &["pull", "--rebase", "--quiet"])
        .await
        .map(drop)
        .map_err(|err| err.to_string());
    if pulled.is_err() {
        let _ = text(clone, &["rebase", "--abort"]).await;
    }
    pulled
}

/// `git fetch` of the clone's upstream.
async fn fetch(clone: &Path) -> Result<(), String> {
    text(clone, &["fetch", "--quiet"])
        .await
        .map(drop)
        .map_err(|err| err.to_string())
}

/// Moves the clone to its upstream as fetched, dropping its own commits:
/// the host keeps their changes and applies them again.
async fn reset(clone: &Path) -> Result<(), String> {
    text(clone, &["reset", "--quiet", "--hard", "@{upstream}"])
        .await
        .map(drop)
        .map_err(|err| err.to_string())
}

/// `git push` of the clone's branch to its upstream, never forced.
async fn push(clone: &Path) -> Result<(), String> {
    text(clone, &["push", "--quiet"])
        .await
        .map(drop)
        .map_err(|err| err.to_string())
}

/// Commits the file at `relative` alone, as Conductor.
async fn commit(
    clone: &Path,
    relative: &str,
    message: &str,
) -> Result<(), String> {
    text(clone, &["add", "--", relative])
        .await
        .map_err(|err| err.to_string())?;
    text(
        clone,
        &[
            "-c",
            "user.name=Conductor",
            "-c",
            "user.email=conductor@localhost",
            "commit",
            "--quiet",
            "-m",
            message,
            "--",
            relative,
        ],
    )
    .await
    .map(drop)
    .map_err(|err| err.to_string())
}

/// Each `.md` file in a project's issues folder, by its name without the
/// extension, with its text.
fn issue_files(folder: &Path) -> Vec<(String, Result<String, String>)> {
    std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let stem = path
                .extension()
                .filter(|extension| *extension == "md")
                .and_then(|_| path.file_stem())?
                .to_string_lossy()
                .into_owned();
            let read =
                std::fs::read_to_string(&path).map_err(|err| err.to_string());
            Some((stem, read))
        })
        .collect()
}

/// Each folder of the clone's `projects/`, with its `project.toml`.
fn folders(clone: &Path) -> Vec<(String, Result<String, String>)> {
    std::fs::read_dir(clone.join("projects"))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .map(|entry| {
            let read =
                std::fs::read_to_string(entry.path().join("project.toml"))
                    .map_err(|err| err.to_string());
            (entry.file_name().to_string_lossy().into_owned(), read)
        })
        .collect()
}

/// A search root with the directories in it and in each of those that
/// isn't hidden, so a checkout one level down is found too.
fn root(path: &Path) -> Root {
    let mut entries = directories(path)
        .into_iter()
        .flat_map(|name| {
            let deeper = if name.starts_with('.') {
                Vec::new()
            } else {
                directories(&path.join(&name))
            };
            let inner = deeper
                .into_iter()
                .map(|inner| format!("{name}/{inner}"))
                .collect::<Vec<String>>();
            core::iter::once(name).chain(inner)
        })
        .collect::<Vec<String>>();
    entries.sort();
    Root {
        path: path.to_string_lossy().into_owned(),
        entries,
    }
}

/// The names of the directories in `path`.
fn directories(path: &Path) -> Vec<String> {
    std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect()
}

async fn probes(paths: Vec<String>) -> Vec<Probe> {
    let mut found = Vec::new();
    for path in paths {
        found.extend(probe(&path).await);
    }
    found
}

/// What Git says of the repository at `path`, by its canonical path.
async fn probe(path: &str) -> Option<Probe> {
    let canonical = std::fs::canonicalize(path).ok()?;
    let shown = canonical.to_str()?.to_owned();
    let top = text(&canonical, &["rev-parse", "--show-toplevel"])
        .await
        .ok();
    let roots = text(&canonical, &["rev-list", "--max-parents=0", "HEAD"])
        .await
        .ok();
    let remote = text(&canonical, &["remote", "get-url", "origin"])
        .await
        .ok();
    home::checkout::probed(
        &shown,
        top.as_deref(),
        roots.as_deref(),
        remote.as_deref(),
    )
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
fn save<T: Serialize + ?Sized>(path: &Path, stored: &T, what: &str) {
    let written = postcard::to_stdvec(stored)
        .map_err(anyhow::Error::from)
        .and_then(|bytes| replace(path, &bytes));
    if let Err(err) = written {
        tracing::warn!(target: "home", %err, "could not store {what}");
    }
}

fn replace(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let next = path.with_extension("new");
    std::fs::write(&next, bytes)?;
    std::fs::rename(&next, path)?;
    Ok(())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}
