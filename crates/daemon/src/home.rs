//! The host's clone of the home (docs/home.md §3) and its projects'
//! checkouts on this host.
//!
//! What the files say, which repositories are
//! checkouts and where the clone stands are the `home` core's decisions;
//! this task runs `git`, reads the directories and keeps what the host
//! stores of its home, and nothing else touches the clone. It never
//! writes in a checkout it found: it only reads them, and clones into its
//! own data dir.
//!
//! One task owns it; [`HomeClone`] handles talk to it over a channel. What
//! the home looks like now is on a watch channel, so a `Welcome` and a
//! subscription read it without waiting on a pull or a clone.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use home::Stored;
use home::checkout::{Probe, Root};
use home::project::Project;
use home::sync::{Git, Sync};
use host::serve::HomeOrder;
use proto::{
    Checkout, Cloning, CommandResult, Home, HostPath, ProjectId, Rejection,
    StartAt,
};
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
    known: Vec<String>,
    shown: watch::Sender<Option<Home>>,
}

impl Actor {
    fn new(homing: Homing, shown: watch::Sender<Option<Home>>) -> Self {
        let Homing {
            data_dir,
            remote,
            roots,
        } = homing;
        let stored_at = data_dir.join("home.state");
        let stored = load(&stored_at);
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
            data_dir,
            stored: stored_at,
            roots,
            sync: Sync::new(),
            projects: Vec::new(),
            found: BTreeMap::new(),
            cloning: BTreeMap::new(),
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

    /// Clones or pulls the home, then reads it again.
    async fn refresh(&mut self) {
        let Some(remote) = self.remote.clone() else {
            return;
        };
        let outcome = match home::sync::next(cloned(&self.clone)) {
            Git::Clone => clone(&self.data_dir, &remote, &self.clone).await,
            Git::Pull => pull(&self.clone).await,
        };
        self.sync =
            self.sync.clone().after(outcome, cloned(&self.clone), now());
        self.scan().await;
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
        save(&self.stored, &stored);
        self.publish();
    }

    fn publish(&self) {
        self.shown
            .send_replace(self.remote.as_deref().map(|remote| {
                home::view(
                    remote,
                    &self.sync,
                    &self.projects,
                    (&self.found, &self.cloning),
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

/// What the host stored of its home, or nothing if it can't be read.
fn load(path: &Path) -> Stored {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| postcard::from_bytes(&bytes).ok())
        .unwrap_or_default()
}

/// Writes the store beside its file, then renames it over, so a crash
/// leaves either.
fn save(path: &Path, stored: &Stored) {
    let written = postcard::to_stdvec(stored)
        .map_err(anyhow::Error::from)
        .and_then(|bytes| replace(path, &bytes));
    if let Err(err) = written {
        tracing::warn!(target: "home", %err, "could not store the home's checkouts");
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
