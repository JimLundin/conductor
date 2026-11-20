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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use home::checkout::{Probe, Root};
use home::instructions::{Own, Sources};
use home::issue::{
    Commit, Context, Delivery, Edit, Listing, Record, RecordEffect,
    RemoteProbe, SessionUpdate, Write,
};
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
    /// The clone, which a session's instructions are read from.
    clone: PathBuf,
}

pub use home::Placement;

type Placed = Result<Placement, Rejection>;

#[derive(Debug)]
enum Msg {
    Tick,
    Order {
        order: HomeOrder,
        reply: oneshot::Sender<CommandResult>,
    },
    Place {
        at: StartAt,
        reply: oneshot::Sender<Placed>,
    },
    Ready {
        start: home::issue::Start,
        reply: oneshot::Sender<Result<(), Rejection>>,
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
        reply: oneshot::Sender<()>,
    },
}

impl HomeClone {
    /// Starts the task. It reads what it has at once, then clones or pulls
    /// the home, and pulls it again every few minutes.
    #[must_use]
    pub fn spawn(homing: Homing) -> Self {
        let (inbox, messages) = mpsc::channel(16);
        let (shown, watched) = watch::channel(None);
        let clone = clone_of(&homing.data_dir);
        tokio::spawn(Actor::new(homing, shown).run(messages));
        Self {
            inbox,
            shown: watched,
            clone,
        }
    }

    /// What a session created in `created`, whose agent runs in
    /// `runs_in`, is told by a CLI that reads `file` by itself: the files
    /// as the clone and the directory it runs in have them now, read
    /// without waiting on the task. Before the task first shows the home,
    /// that is the shared rules alone.
    #[must_use]
    pub fn instructions(
        &self,
        created: &HostPath,
        runs_in: &HostPath,
        file: &str,
    ) -> Option<String> {
        let projects = self.shown().map_or_default(|home| home.projects);
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
    pub async fn order(
        &self,
        order: HomeOrder,
    ) -> Result<CommandResult, Stopped> {
        self.ask(|reply| Msg::Order { order, reply }).await
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
    ) -> Result<Result<(), Rejection>, Stopped> {
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
    ) -> Result<(), Stopped> {
        self.ask(|reply| Msg::Finished {
            issue,
            summary,
            client,
            landed,
            reply,
        })
        .await
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

/// An issue change the remote doesn't have yet, and whether the clone's
/// branch holds it now, so a push lands it.
struct Pending {
    edit: Edit,
    written: bool,
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
    edits: Vec<Pending>,
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
            clone: clone_of(&data_dir),
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
            edits: load_edits(&edits_at)
                .into_iter()
                .map(|edit| Pending {
                    edit,
                    written: false,
                })
                .collect(),
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
            Msg::Place { at, reply } => {
                self.fresh().await;
                let shown = self.shown.borrow().clone();
                let _ = reply.send(home::place(at, shown.as_ref()));
            }
            Msg::Ready { start, reply } => {
                let readied = ready_branch(&start)
                    .await
                    .map_err(|reason| Rejection::WorkspaceFailure { reason });
                let _ = reply.send(readied);
            }
            Msg::Started {
                issue,
                client,
                reply,
            } => {
                self.session_update(issue, &client, SessionUpdate::Started)
                    .await;
                let _ = reply.send(());
            }
            Msg::Finished {
                issue,
                summary,
                client,
                landed,
                reply,
            } => {
                self.session_update(
                    issue,
                    &client,
                    SessionUpdate::Finished { summary, landed },
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
    ) {
        while let Some(change) = update.next(&self.issues, issue) {
            let answer = self.change(issue, change, client).await;
            if answer != CommandResult::Accepted {
                tracing::warn!(target: "home", ?answer, "a session's issue update was rejected");
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

    /// Replays the kept changes once the remote has been read; how the
    /// read or the replay's push went.
    async fn replay_after(
        &mut self,
        read: Result<(), String>,
    ) -> Result<(), String> {
        read?;
        self.deliver(Delivery::replay(!self.edits.is_empty())).await
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
        let issue = IssueId(ulid::Ulid::new().0);
        let request = self
            .issue_context()
            .open(&projects, project, issue, &wanted);
        match self.record(request).await {
            Ok(()) => CommandResult::IssueCreated { issue },
            Err(rejection) => CommandResult::Rejected(rejection),
        }
    }

    async fn change(
        &mut self,
        issue: IssueId,
        change: IssueChange,
        client: &ClientName,
    ) -> CommandResult {
        self.fresh().await;
        let request = self.issue_context().edit(issue, change, &client.0);
        home::answer(self.record(request).await)
    }

    fn issue_context(&self) -> Context<'_> {
        Context {
            remote: self.remote.as_deref(),
            issues: &self.issues,
            host: &self.host,
            at: now(),
        }
    }

    async fn record(
        &mut self,
        decision: Result<Record, Rejection>,
    ) -> Result<(), Rejection> {
        let request = decision?;
        let current =
            std::fs::read_to_string(self.clone.join(request.read().path)).ok();
        let mut recorded = request.decide(current.as_deref())?;
        let mut outcome = Ok(());
        while let Some(effect) = recorded.next(outcome)? {
            outcome = match effect {
                RecordEffect::Commit(commit) => self.commit(commit).await,
                RecordEffect::Enqueue(edit) => {
                    self.edits.push(Pending {
                        edit,
                        written: true,
                    });
                    Ok(())
                }
                RecordEffect::Persist => {
                    self.keep();
                    Ok(())
                }
                RecordEffect::Deliver(delivery) => self.deliver(delivery).await,
                RecordEffect::Scan => {
                    self.scan().await;
                    Ok(())
                }
            };
        }
        Ok(())
    }

    async fn deliver(&mut self, mut step: Delivery) -> Result<(), String> {
        loop {
            let outcome = match step {
                Delivery::Push { .. } => push(&self.clone).await,
                Delivery::Fetch => fetch(&self.clone).await,
                Delivery::Reset => reset(&self.clone).await,
                Delivery::Rewrite => {
                    self.rewrite_all().await;
                    Ok(())
                }
                Delivery::Persist => {
                    self.keep();
                    Ok(())
                }
                Delivery::Landed => {
                    self.landed();
                    Ok(())
                }
                Delivery::Done => return Ok(()),
            };
            step = step.after(outcome.is_ok());
            if step == Delivery::Done {
                return outcome;
            }
        }
    }

    async fn rewrite_all(&mut self) {
        let mut kept = Vec::new();
        for pending in core::mem::take(&mut self.edits) {
            kept.extend(self.rewrite(pending.edit).await);
        }
        self.edits = kept;
    }

    /// Applies a kept change again on top of the remote, and keeps it
    /// unless its issue is gone; one that can't be written waits.
    async fn rewrite(&self, edit: Edit) -> Option<Pending> {
        let read = edit.read();
        let changed = changed_at(&self.clone, &read.path).await;
        let current = std::fs::read_to_string(self.clone.join(&read.path)).ok();
        let outcome = match edit.replay(current.as_deref(), changed) {
            Write::Unchanged => Ok(()),
            Write::Dropped => {
                tracing::warn!(target: "home", issue = ?read.issue, "dropped a change to an issue the home no longer has");
                return None;
            }
            Write::Waiting(reason) => Err(reason),
            Write::Commit(commit) => self.commit(commit).await,
        };
        if let Err(reason) = &outcome {
            tracing::warn!(target: "home", %reason, "an issue change waits");
        }
        Some(Pending {
            edit,
            written: outcome.is_ok(),
        })
    }

    /// The remote has every change written in the clone.
    fn landed(&mut self) {
        self.edits.retain(|pending| !pending.written);
        self.keep();
    }

    fn keep(&self) {
        let edits = self
            .edits
            .iter()
            .map(|pending| pending.edit.clone())
            .collect::<Vec<Edit>>();
        save(&self.edits_at, &edits, "the unpushed issue changes");
    }

    async fn commit(&self, commit: Commit) -> Result<(), String> {
        let path = self.clone.join(&commit.target.path);
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder).map_err(|err| err.to_string())?;
        }
        std::fs::write(path, &commit.text).map_err(|err| err.to_string())?;
        commit_file(&self.clone, &commit).await
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
                let listing = Listing::for_project(&project.id);
                let files = issue_files(&self.clone.join(&listing.path));
                let (issues, problems) = listing.decide(files);
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
        home::issue::Remote::Absent | home::issue::Remote::Unreachable(_) => {
            None
        }
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
            RemoteProbe::Reachability => {
                text(checkout, &["ls-remote", "--quiet", "origin"]).await
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

async fn fetch(clone: &Path) -> Result<(), String> {
    run(clone, &["fetch", "--quiet"]).await
}

/// Moves the clone to its upstream as fetched, dropping its own commits:
/// the host keeps their changes and applies them again.
async fn reset(clone: &Path) -> Result<(), String> {
    run(clone, &["reset", "--quiet", "--hard", "@{upstream}"]).await
}

/// Pushes the clone's branch to its upstream, never forced.
async fn push(clone: &Path) -> Result<(), String> {
    run(clone, &["push", "--quiet"]).await
}

async fn run(clone: &Path, args: &[&str]) -> Result<(), String> {
    text(clone, args)
        .await
        .map(drop)
        .map_err(|err| err.to_string())
}

/// When the clone's branch last changed the file, in seconds since the
/// Unix epoch; 0 when it never did.
async fn changed_at(clone: &Path, relative: &str) -> u64 {
    text(clone, &["log", "-1", "--format=%ct", "--", relative])
        .await
        .ok()
        .and_then(|seconds| seconds.trim().parse().ok())
        .unwrap_or(0)
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

/// The unpushed issue changes the host kept. A store that no longer reads
/// is moved aside, so its changes are not lost unseen.
fn load_edits(path: &Path) -> Vec<Edit> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    postcard::from_bytes(&bytes).unwrap_or_else(|err| {
        let aside = path.with_extension("unreadable");
        let moved = std::fs::rename(path, &aside);
        tracing::warn!(target: "home", %err, aside = %aside.display(), ?moved, "could not read the unpushed issue changes");
        Vec::new()
    })
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
