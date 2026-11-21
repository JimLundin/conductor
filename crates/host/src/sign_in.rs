//! The app's sign-ins (architecture §6.3).
//!
//! One runs per instance at a time, and a second is refused until the
//! first ends. Each runs the CLI's own sign-in in the instance's home,
//! reads what to show the person from its output as the provider's plugin
//! describes, sends a pasted code to its stdin, and once it exits 0 checks
//! the home, reads the account the CLI says it signed in as, and refreshes
//! the instance, which becomes that account's (`registry`).
//!
//! Every step goes to the clients live and is stored nowhere: the code,
//! the output and the link are in no event, receipt or log line.
//!
//! `daemon` runs the CLI and performs each [`Effect`]; it reports what the
//! run did, by its [`Key`], and a report of a run since ended is dropped.

use alloc::collections::BTreeMap;
use alloc::format;
use core::time::Duration;

use agents::{SignIn as Flow, plain};
use proto::{
    Account, CommandResult, Instance, InstanceId, Rejection, SignIn,
    SignInCode, SignInStep,
};

use crate::Note;
use crate::prelude::*;
use crate::registry::AccountAdoption;

/// How long a sign-in may run before the host stops it: Codex's one-time
/// code expires after 15 minutes.
pub const LIMIT: Duration = Duration::from_mins(15);

/// The most output lines kept while the link isn't shown yet.
const KEPT: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Run(pub u64);

/// One run of an instance's sign-in.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key {
    pub instance: InstanceId,
    pub run: Run,
}

/// A login process that completed successfully.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Login(());

impl Login {
    /// # Errors
    ///
    /// The sign-in process did not exit successfully.
    pub fn exited(code: Option<i32>, status: &str) -> Result<Self, String> {
        if code == Some(0) {
            Ok(Self(()))
        } else {
            Err(format!("the sign-in exited with {status}"))
        }
    }

    /// Releases the probe's output for account discovery.
    ///
    /// # Errors
    ///
    /// The probe did not exit successfully.
    pub fn checked(
        self,
        probe: Result<(Option<i32>, String), String>,
    ) -> Result<String, String> {
        match probe {
            Ok((Some(0), output)) => Ok(output),
            Ok(_) | Err(_) => Err(String::from(
                "the sign-in ended, but the CLI's sign-in check still fails",
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Start the CLI's sign-in in the instance's home, as its `settings`
    /// say, with its output and stdin on pipes. Report [`SignIns::spawned`],
    /// each line it prints to [`SignIns::printed`] and its exit to
    /// [`SignIns::exited`], or [`SignIns::expired`] once it has run for
    /// [`LIMIT`].
    Spawn {
        key: Key,
        settings: Instance,
    },
    /// Write the code to the run's stdin, as one line.
    Write(Key, SignInCode),
    Kill(Key),
    /// Run the CLI's sign-in check against the home, read the account the
    /// CLI names, and report both to [`SignIns::checked`].
    Check(Key, Login),
    /// Take the instance as signed in as the account, if the CLI named
    /// one, and refresh it, and report what the host runs then to
    /// [`SignIns::listed`].
    Refresh(Key, Option<Account>),
    /// Send this to every subscribed client.
    Publish(SignIn),
}

#[derive(Clone, Default)]
pub struct SignIns {
    running: BTreeMap<InstanceId, Running>,
    runs: u64,
}

#[derive(Clone)]
struct Running {
    run: Run,
    /// How to read the run's output, once it has started.
    flow: Option<Flow>,
    /// Its output, until it shows the link.
    seen: Vec<String>,
    step: SignInStep,
    /// What the output said a failure means.
    failure: Option<String>,
}

impl SignIns {
    #[must_use]
    pub fn is_running(&self, instance: &InstanceId) -> bool {
        self.running.contains_key(instance)
    }

    /// Starts a sign-in on `instance`, whose settings are `known` if the
    /// host has it.
    pub fn start(
        &mut self,
        instance: InstanceId,
        known: Option<Instance>,
    ) -> (CommandResult, Vec<Effect>) {
        let Some(settings) = known else {
            return (rejected(Rejection::UnknownInstance), Vec::new());
        };
        let Ok(key) = self.begin(instance.clone(), SignInStep::Starting) else {
            return (rejected(Rejection::SignInRunning), Vec::new());
        };
        (
            CommandResult::Accepted,
            Vec::from([
                Effect::Spawn { key, settings },
                publish(instance, SignInStep::Starting),
            ]),
        )
    }

    /// Waits for the refresh after recovering a confirmed sign-in.
    ///
    /// # Errors
    ///
    /// The instance already has a sign-in running.
    pub fn resume(
        &mut self,
        requested: &InstanceId,
        adoption: &AccountAdoption,
    ) -> Result<Option<Key>, Rejection> {
        adoption
            .destination_for(requested)
            .map(|_| self.begin(requested.clone(), SignInStep::Finishing))
            .transpose()
    }

    fn begin(
        &mut self,
        instance: InstanceId,
        step: SignInStep,
    ) -> Result<Key, Rejection> {
        if self.running.contains_key(&instance) {
            return Err(Rejection::SignInRunning);
        }
        self.runs = self.runs.saturating_add(1);
        let run = Run(self.runs);
        self.running.insert(
            instance.clone(),
            Running {
                run,
                flow: None,
                seen: Vec::new(),
                step,
                failure: None,
            },
        );
        Ok(Key { instance, run })
    }

    /// Sends the pasted code to a sign-in that waits for one.
    pub fn submit(
        &mut self,
        instance: InstanceId,
        code: SignInCode,
    ) -> (CommandResult, Vec<Effect>) {
        match self.running.get_mut(&instance) {
            Some(running)
                if matches!(
                    running.step,
                    SignInStep::Open { paste: true, .. }
                ) =>
            {
                running.step = SignInStep::Finishing;
                let key = Key {
                    instance: instance.clone(),
                    run: running.run,
                };
                (
                    CommandResult::Accepted,
                    Vec::from([
                        Effect::Write(key, code),
                        publish(instance, SignInStep::Finishing),
                    ]),
                )
            }
            Some(_) | None => (rejected(Rejection::NoCodeAsked), Vec::new()),
        }
    }

    /// Stops the instance's sign-in, if one runs.
    pub fn cancel(
        &mut self,
        instance: InstanceId,
    ) -> (CommandResult, Vec<Effect>) {
        let effects =
            self.running
                .remove(&instance)
                .map_or_else(Vec::new, |running| {
                    let key = Key {
                        instance: instance.clone(),
                        run: running.run,
                    };
                    Vec::from([
                        Effect::Kill(key),
                        publish(instance, SignInStep::Cancelled),
                    ])
                });
        (CommandResult::Accepted, effects)
    }

    /// The run started, reading its output as `flow` says, or why not.
    pub fn spawned(
        &mut self,
        key: &Key,
        flow: Result<Flow, String>,
    ) -> Vec<Effect> {
        let Some(running) = self.current(key) else {
            return Vec::new();
        };
        match flow {
            Ok(described) => {
                running.flow = Some(described);
                Vec::new()
            }
            Err(reason) => self.fail(key, reason),
        }
    }

    /// The run printed `line`, on its stdout or its stderr.
    pub fn printed(&mut self, key: &Key, line: &str) -> Vec<Effect> {
        self.current(key)
            .and_then(|running| running.read(plain(line)))
            .map(|step| publish(key.instance.clone(), step))
            .into_iter()
            .collect()
    }

    /// The run exited with `code`, which the OS describes as `status`.
    pub fn exited(
        &mut self,
        key: &Key,
        code: Option<i32>,
        status: &str,
    ) -> Vec<Effect> {
        let Some(running) = self.current(key) else {
            return Vec::new();
        };
        match Login::exited(code, status) {
            Ok(login) => {
                running.step = SignInStep::Finishing;
                Vec::from([
                    Effect::Check(key.clone(), login),
                    publish(key.instance.clone(), SignInStep::Finishing),
                ])
            }
            Err(exit_reason) => {
                let reason = running.failure.clone().unwrap_or(exit_reason);
                self.fail(key, reason)
            }
        }
    }

    /// The run has had its [`LIMIT`].
    pub fn expired(&mut self, key: &Key) -> Vec<Effect> {
        if self.current(key).is_none() {
            return Vec::new();
        }
        let minutes = LIMIT.as_secs() / 60;
        let mut effects = Vec::from([Effect::Kill(key.clone())]);
        effects.extend(self.fail(
            key,
            format!("the sign-in took longer than {minutes} minutes"),
        ));
        effects
    }

    /// The completed login confirmed an account, if the CLI named one.
    pub fn checked(
        &mut self,
        key: &Key,
        result: Result<Option<Account>, String>,
    ) -> Vec<Effect> {
        if self.current(key).is_none() {
            return Vec::new();
        }
        match result {
            Ok(account) => Vec::from([Effect::Refresh(key.clone(), account)]),
            Err(reason) => self.fail(key, reason),
        }
    }

    /// The instance refresh completed, or stopped without an answer.
    pub fn listed(
        &mut self,
        key: &Key,
        result: Option<CommandResult>,
    ) -> Vec<Effect> {
        if self.current(key).is_none() {
            return Vec::new();
        }
        self.running.remove(&key.instance);
        let step = match result {
            Some(CommandResult::Accepted) => SignInStep::SignedIn,
            other => SignInStep::Failed {
                reason: format!(
                    "the host could not refresh the instance: {other:?}"
                ),
            },
        };
        Vec::from([publish(key.instance.clone(), step)])
    }

    /// Where each running sign-in stands, which a new subscription starts
    /// with.
    #[must_use]
    pub fn running(&self) -> Vec<SignIn> {
        self.running
            .iter()
            .map(|(instance, running)| SignIn {
                instance: instance.clone(),
                step: running.step.clone(),
            })
            .collect()
    }

    /// The instance's sign-in, if `key` is its run.
    fn current(&mut self, key: &Key) -> Option<&mut Running> {
        self.running
            .get_mut(&key.instance)
            .filter(|running| running.run == key.run)
    }

    fn fail(&mut self, key: &Key, reason: String) -> Vec<Effect> {
        self.running.remove(&key.instance);
        Vec::from([publish(
            key.instance.clone(),
            SignInStep::Failed { reason },
        )])
    }
}

impl Running {
    /// Reads a line of output: a failure it names, and once the link and
    /// any code are there, the step that shows them.
    fn read(&mut self, text: String) -> Option<SignInStep> {
        let flow = self.flow.as_ref()?;
        if let Some(means) = flow.failure(&text) {
            self.failure = Some(String::from(means));
        }
        if self.step != SignInStep::Starting {
            return None;
        }
        if self.seen.len() >= KEPT {
            self.seen.remove(0);
        }
        self.seen.push(text);
        let shown = flow.shown(&self.seen)?;
        self.seen.clear();
        self.step = SignInStep::Open {
            url: shown.link,
            code: shown.code,
            paste: flow.paste,
        };
        Some(self.step.clone())
    }
}

/// Releases the private home when a sign-in reaches a terminal step.
#[must_use]
pub const fn release(sign_in: &SignIn) -> Option<&InstanceId> {
    match sign_in.step {
        SignInStep::SignedIn
        | SignInStep::Failed { .. }
        | SignInStep::Cancelled => Some(&sign_in.instance),
        SignInStep::Starting
        | SignInStep::Open { .. }
        | SignInStep::Finishing => None,
    }
}

/// What the host logs of a step: its name, and why a failed one failed,
/// but never its link or code.
#[must_use]
pub fn note(sign_in: &SignIn) -> Note {
    let step = match &sign_in.step {
        SignInStep::Starting => String::from("starting"),
        SignInStep::Open { .. } => String::from("open"),
        SignInStep::Finishing => String::from("finishing"),
        SignInStep::SignedIn => String::from("signed in"),
        SignInStep::Failed { reason } => format!("failed: {reason}"),
        SignInStep::Cancelled => String::from("cancelled"),
    };
    Note::SignIn {
        instance: sign_in.instance.clone(),
        step,
    }
}

const fn rejected(rejection: Rejection) -> CommandResult {
    CommandResult::Rejected(rejection)
}

const fn publish(instance: InstanceId, step: SignInStep) -> Effect {
    Effect::Publish(SignIn { instance, step })
}

#[cfg(test)]
mod tests;
