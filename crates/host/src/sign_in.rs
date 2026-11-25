use alloc::collections::BTreeMap;
use alloc::format;
use core::time::Duration;

use crate::registry::{
    Availability, Catalog, Instance, InstanceId, InstanceRefusal,
    InstanceResult, Instances,
};
use agents::{Account, Auth, Credential, SignInCode};
use agents::{SignIn as Flow, plain};

use crate::Note;
use crate::prelude::*;
use crate::registry::AccountAdoption;

mod model;
pub use model::{
    SignIn, SignInCommand, SignInRefusal, SignInResult, SignInStep,
};

pub const LIMIT: Duration = Duration::from_mins(15);

const OUTPUT_LINE_LIMIT: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Run(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key {
    pub instance: InstanceId,
    pub run: Run,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Login(());

impl Login {
    pub fn exited(code: Option<i32>, status: &str) -> Result<Self, String> {
        if code == Some(0) {
            Ok(Self(()))
        } else {
            Err(format!("the sign-in exited with {status}"))
        }
    }

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
pub struct CredentialTarget {
    pub destination: InstanceId,
    pub kind: String,
}

impl CredentialTarget {
    pub fn requested(
        catalog: &Catalog,
        source: &InstanceId,
        kind: String,
        label: &str,
    ) -> Result<Self, SignInRefusal> {
        let account = label.trim();
        if source.account != Account::Own
            || account.is_empty()
            || account.len() > 256
            || account.chars().any(char::is_control)
        {
            return Err(SignInRefusal::InvalidCredential {
                reason: String::from(
                    "a credential sign-in needs a valid account label",
                ),
            });
        }
        if catalog
            .of(&source.provider)
            .and_then(|info| info.form(&kind))
            .is_none_or(|form| form.setup_sign_in.is_none())
        {
            return Err(SignInRefusal::InvalidCredential {
                reason: String::from(
                    "this credential kind does not support sign-in",
                ),
            });
        }
        Ok(Self {
            destination: InstanceId::named(source.provider.clone(), account),
            kind,
        })
    }

    #[must_use]
    pub fn accepts(&self, catalog: &Catalog, credential: &Credential) -> bool {
        catalog.of(&self.destination.provider).is_some_and(|info| {
            credential.kind == self.kind
                && info
                    .account(credential)
                    .is_ok_and(|account| account == self.destination.account)
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Spawn { key: Key, settings: Instance },
    SpawnCredential(Key, CredentialTarget),
    StoreCredential(Key, CredentialTarget, Credential),
    Write(Key, SignInCode),
    Kill(Key),
    Check(Key, Login),
    Refresh(Key, Option<Account>),
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
    flow: Option<Flow>,
    seen: Vec<String>,
    step: SignInStep,
    failure: Option<String>,
    committing: bool,
    credential: Option<CredentialTarget>,
}

impl SignIns {
    #[must_use]
    pub fn is_running(&self, instance: &InstanceId) -> bool {
        self.running.contains_key(instance)
    }

    pub fn start(
        &mut self,
        instance: InstanceId,
        known: Option<Instance>,
    ) -> (SignInResult, Vec<Effect>) {
        let Some(settings) = known else {
            return (refuse(SignInRefusal::UnknownInstance), Vec::new());
        };
        let Ok(key) = self.begin(instance.clone(), SignInStep::Starting, None)
        else {
            return (refuse(SignInRefusal::SignInRunning), Vec::new());
        };
        (
            SignInResult::Accepted,
            Vec::from([
                Effect::Spawn { key, settings },
                publish(instance, SignInStep::Starting),
            ]),
        )
    }

    pub fn start_credential(
        &mut self,
        instance: InstanceId,
        target: CredentialTarget,
    ) -> (SignInResult, Vec<Effect>) {
        let Ok(key) = self.begin(
            instance.clone(),
            SignInStep::Starting,
            Some(target.clone()),
        ) else {
            return (refuse(SignInRefusal::SignInRunning), Vec::new());
        };
        (
            SignInResult::Accepted,
            Vec::from([
                Effect::SpawnCredential(key, target),
                publish(instance, SignInStep::Starting),
            ]),
        )
    }

    pub fn credential_link(&mut self, key: &Key, url: String) -> Vec<Effect> {
        let Some(running) = self.current(key).filter(|running| {
            running.credential.is_some() && running.step == SignInStep::Starting
        }) else {
            return Vec::new();
        };
        running.step = SignInStep::Open {
            url,
            code: None,
            paste: true,
        };
        Vec::from([publish(key.instance.clone(), running.step.clone())])
    }

    pub fn credential_created(
        &mut self,
        key: &Key,
        created: Option<Credential>,
    ) -> Vec<Effect> {
        let Some(running) =
            self.current(key).filter(|running| !running.committing)
        else {
            return Vec::new();
        };
        let Some(target) = running.credential.clone() else {
            return Vec::new();
        };
        if let Some(credential) = created.filter(|value| {
            running.step == SignInStep::Finishing && value.kind == target.kind
        }) {
            running.committing = true;
            Vec::from([Effect::StoreCredential(
                key.clone(),
                target,
                credential,
            )])
        } else {
            self.fail(
                key,
                String::from("the provider could not create the credential"),
            )
        }
    }

    pub fn resume(
        &mut self,
        requested: &InstanceId,
        adoption: &AccountAdoption,
    ) -> Result<Option<Key>, SignInRefusal> {
        adoption
            .destination_for(requested)
            .map(|_| {
                let key =
                    self.begin(requested.clone(), SignInStep::Finishing, None)?;
                if let Some(running) = self.current(&key) {
                    running.committing = true;
                }
                Ok(key)
            })
            .transpose()
    }

    fn begin(
        &mut self,
        instance: InstanceId,
        step: SignInStep,
        credential: Option<CredentialTarget>,
    ) -> Result<Key, SignInRefusal> {
        if self.running.contains_key(&instance) {
            return Err(SignInRefusal::SignInRunning);
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
                committing: false,
                credential,
            },
        );
        Ok(Key { instance, run })
    }

    pub fn submit(
        &mut self,
        instance: InstanceId,
        code: SignInCode,
    ) -> (SignInResult, Vec<Effect>) {
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
                    SignInResult::Accepted,
                    Vec::from([
                        Effect::Write(key, code),
                        publish(instance, SignInStep::Finishing),
                    ]),
                )
            }
            Some(_) | None => (refuse(SignInRefusal::NoCodeAsked), Vec::new()),
        }
    }

    pub fn cancel(
        &mut self,
        instance: InstanceId,
    ) -> (SignInResult, Vec<Effect>) {
        if self
            .running
            .get(&instance)
            .is_some_and(|run| run.committing)
        {
            return (SignInResult::Accepted, Vec::new());
        }
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
        (SignInResult::Accepted, effects)
    }

    pub fn write_failed(&mut self, key: &Key, reason: String) -> Vec<Effect> {
        if self.current(key).is_some() {
            let mut effects = Vec::from([Effect::Kill(key.clone())]);
            effects.extend(self.fail(key, reason));
            effects
        } else {
            Vec::new()
        }
    }

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

    pub fn printed(&mut self, key: &Key, line: &str) -> Vec<Effect> {
        self.current(key)
            .and_then(|running| running.read(plain(line)))
            .map(|step| publish(key.instance.clone(), step))
            .into_iter()
            .collect()
    }

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

    pub fn checked(
        &mut self,
        key: &Key,
        result: Result<Option<Account>, String>,
    ) -> Vec<Effect> {
        let Some(running) = self.current(key) else {
            return Vec::new();
        };
        match result {
            Ok(account) => {
                running.committing = true;
                Vec::from([Effect::Refresh(key.clone(), account)])
            }
            Err(reason) => self.fail(key, reason),
        }
    }

    pub fn listed(
        &mut self,
        key: &Key,
        result: Option<InstanceResult>,
        instances: &Instances,
    ) -> Vec<Effect> {
        let Some(running) = self.current(key) else {
            return Vec::new();
        };
        let credential = running.credential.take();
        self.running.remove(&key.instance);
        let step = match (credential, result) {
            (expected, Some(InstanceResult::Accepted))
                if expected.as_ref().is_none_or(|target| {
                    matches!(
                        instances.availability(&target.destination),
                        Some(Availability::Available { auth: Auth::Stored { kind, .. }, .. })
                            if *kind == target.kind
                    )
                }) => SignInStep::SignedIn,
            (Some(_), Some(InstanceResult::Refused(InstanceRefusal::FleetUnavailable))) => SignInStep::Failed {
                reason: String::from("the fleet could not save the credential"),
            },
            (Some(_), _) => SignInStep::Failed {
                reason: String::from("the host could not save and refresh the credential"),
            },
            (None, other) => SignInStep::Failed {
                reason: format!(
                    "the host could not refresh the instance: {other:?}"
                ),
            },
        };
        Vec::from([publish(key.instance.clone(), step)])
    }

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
    fn read(&mut self, text: String) -> Option<SignInStep> {
        let flow = self.flow.as_ref()?;
        if let Some(means) = flow.failure(&text) {
            self.failure = Some(String::from(means));
        }
        if self.step != SignInStep::Starting {
            return None;
        }
        if self.seen.len() >= OUTPUT_LINE_LIMIT {
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

const fn refuse(refusal: SignInRefusal) -> SignInResult {
    SignInResult::Refused(refusal)
}

const fn publish(instance: InstanceId, step: SignInStep) -> Effect {
    Effect::Publish(SignIn { instance, step })
}

#[cfg(test)]
mod tests;
