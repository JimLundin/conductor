//! Runs the app's sign-ins.
//!
//! Each is the CLI's own sign-in in its instance's home, on pipes, then
//! its sign-in check and the account the CLI names, then the refresh's
//! answer. The decisions are
//! [`host::sign_in`]'s; the registry's actor performs them with [`Runs`]
//! and hands the core what each run reports.
//!
//! Nothing here keeps or logs what a CLI prints or what a person pastes:
//! a printed line goes to the core, and a code to the CLI's stdin.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::process::ExitStatus;

use agents::{Invocation, Reported, SignIn as Flow};
use host::sign_in::{Key, LIMIT, Login};
use install::Home;
use install::process::{SignInRun, command, sign_in, write_line};
use proto::{Account, CommandResult, Instance, SignInCode};
use tokio::process::ChildStdin;

use crate::credentials::SigningLease;
use crate::start::Providers;
use tokio::sync::{mpsc, oneshot};
use tokio::task::{AbortHandle, JoinSet};

/// How an instance's CLI signs its home in: the CLI, the home, the
/// plugin's description of the sign-in, the CLI's sign-in check, and
/// where the CLI names the account and how the plugin reads it.
#[derive(Debug, Clone)]
pub struct Signing {
    pub program: PathBuf,
    pub home: Home,
    pub flow: Flow,
    pub check: Invocation,
    pub account: Reported,
    pub read: fn(&str) -> Option<String>,
}

impl Signing {
    /// # Errors
    ///
    /// The CLI's status probe did not exit successfully.
    pub async fn confirm(
        &self,
        login: Login,
    ) -> Result<Option<Account>, String> {
        let check = command(&self.program, &self.home, self.check.clone());
        let status = login.checked(crate::start::sign_in_check(check).await)?;
        let account = match (&self.account, &self.home) {
            (Reported::Status, _) => (self.read)(&status),
            (Reported::File(file), Home::Private(dir)) => {
                std::fs::read_to_string(dir.join(file))
                    .ok()
                    .and_then(|kept| (self.read)(&kept))
            }
            (Reported::File(_), Home::User) => None,
        };
        Ok(account.map(Account::Named))
    }
}

/// A sign-in that started: how it signs in, and its stdin.
pub struct Spawned {
    signing: Signing,
    stdin: ChildStdin,
}

/// What a run tells the registry.
pub enum Report {
    Spawned(Result<Box<Spawned>, String>),
    Printed(String),
    Exited(Option<i32>, String),
    Expired,
    Checked(Result<Option<Account>, String>),
    Listed(CommandResult),
}

/// The sign-ins' tasks, each running sign-in's stdin and check, and what
/// they report. Dropping it stops every task, which kills its CLI.
pub struct Runs {
    tasks: JoinSet<()>,
    aborts: BTreeMap<Key, AbortHandle>,
    stdins: BTreeMap<Key, ChildStdin>,
    checks: BTreeMap<Key, Signing>,
    reports: mpsc::UnboundedReceiver<(Key, Report)>,
    sender: mpsc::UnboundedSender<(Key, Report)>,
}

impl Default for Runs {
    fn default() -> Self {
        let (sender, reports) = mpsc::unbounded_channel();
        Self {
            tasks: JoinSet::new(),
            aborts: BTreeMap::new(),
            stdins: BTreeMap::new(),
            checks: BTreeMap::new(),
            reports,
            sender,
        }
    }
}

impl Runs {
    /// The next report of any run.
    pub async fn next(&mut self) -> Option<(Key, Report)> {
        self.reports.recv().await
    }

    /// Starts the sign-in `key` of an instance whose settings are
    /// `settings`. What earlier runs of the instance left goes.
    pub fn spawn(
        &mut self,
        key: Key,
        providers: Providers,
        settings: Instance,
        claimed: Option<SigningLease>,
    ) {
        let Some(lease) = claimed else { return };
        while self.tasks.try_join_next().is_some() {}
        let other = |kept: &Key| kept.instance != key.instance;
        self.aborts.retain(|kept, _| other(kept));
        self.stdins.retain(|kept, _| other(kept));
        self.checks.retain(|kept, _| other(kept));
        let task = run(key.clone(), providers, settings, self.sender.clone());
        self.aborts.insert(
            key,
            self.tasks.spawn(async move {
                let _lease = lease;
                task.await;
            }),
        );
    }

    /// Keeps what a run that started needs later, and hands back how to
    /// read its output.
    ///
    /// # Errors
    ///
    /// Why the run didn't start.
    pub fn started(
        &mut self,
        key: &Key,
        spawned: Result<Box<Spawned>, String>,
    ) -> Result<Flow, String> {
        spawned.map(|run| {
            let Spawned { signing, stdin } = *run;
            let flow = signing.flow.clone();
            self.stdins.insert(key.clone(), stdin);
            self.checks.insert(key.clone(), signing);
            flow
        })
    }

    /// Writes the code to the run's stdin. A CLI that has gone reports its
    /// exit.
    pub async fn write(&mut self, key: &Key, code: &SignInCode) {
        if let Some(stdin) = self.stdins.get_mut(key) {
            let _ = write_line(stdin, &code.0).await;
        }
    }

    pub fn kill(&mut self, key: &Key) {
        self.aborts.remove(key).iter().for_each(AbortHandle::abort);
        self.stdins.remove(key);
        self.checks.remove(key);
    }

    /// Runs the CLI's sign-in check against the run's home, and reads
    /// the account it names.
    pub fn check(
        &mut self,
        key: Key,
        login: Login,
        claimed: Option<SigningLease>,
    ) {
        self.stdins.remove(&key);
        if let Some((signing, lease)) = self.checks.remove(&key).zip(claimed) {
            let sender = self.sender.clone();
            let checked = key.clone();
            let task = self.tasks.spawn(async move {
                let result = signing.confirm(login).await;
                drop(lease);
                let _ = sender.send((checked, Report::Checked(result)));
            });
            self.aborts.insert(key, task);
        }
    }

    /// Reports the refresh's answer once it comes.
    pub fn listen(
        &mut self,
        key: Key,
        answer: oneshot::Receiver<CommandResult>,
    ) {
        let sender = self.sender.clone();
        self.tasks.spawn(async move {
            let result = answer.await.unwrap_or(CommandResult::Accepted);
            let _ = sender.send((key, Report::Listed(result)));
        });
    }
}

/// Resolves how the instance signs in, runs the sign-in, and reports it
/// started, each line it prints and how it ended.
async fn run(
    key: Key,
    providers: Providers,
    settings: Instance,
    sender: mpsc::UnboundedSender<(Key, Report)>,
) {
    let report = |report| {
        let _ = sender.send((key.clone(), report));
    };
    match started(&providers, &key, settings).await {
        Err(reason) => report(Report::Spawned(Err(reason))),
        Ok((signing, process, stdin)) => {
            report(Report::Spawned(Ok(Box::new(Spawned { signing, stdin }))));
            let printed = |line| report(Report::Printed(line));
            let ended = tokio::time::timeout(LIMIT, process.finish(printed));
            report(ended.await.map_or(Report::Expired, exited));
        }
    }
}

async fn started(
    providers: &Providers,
    key: &Key,
    settings: Instance,
) -> Result<(Signing, SignInRun, ChildStdin), String> {
    let signing = providers.signing(&key.instance, &settings).await?;
    let (process, stdin) =
        sign_in(&signing.program, &signing.home, signing.flow.run.clone())
            .map_err(|err| {
                format!("{} does not run: {err}", signing.program.display())
            })?;
    Ok((signing, process, stdin))
}

fn exited(status: io::Result<ExitStatus>) -> Report {
    status.map_or_else(
        |err| Report::Exited(None, err.to_string()),
        |ran| Report::Exited(ran.code(), ran.to_string()),
    )
}
