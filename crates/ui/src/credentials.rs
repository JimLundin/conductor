//! Each host's instances and their credentials: what each instance signs in
//! with, its CLI's own sign-in run from the app, a button to remove it,
//! and per provider a form for each kind of credential its plugin
//! describes. An instance is an account of its provider, which the
//! provider names: a credential's form names it, and a sign-in in the
//! provider's own instance makes the instance of the account it signed in
//! as. A secret only passes through: its field is cleared once it is sent,
//! and nothing keeps it (§6.3). A pasted sign-in code is one.

use alloc::collections::BTreeMap;

use crate::prelude::*;
use proto::{
    Account, Auth, Availability, Catalog, CommandBody, CommandResult,
    Credential, CredentialForm, Field, FieldKind, InstanceId, InstanceStatus,
    Instances, Provider, ProviderInfo, SignInCode, SignInStep,
};

use crate::host::{Host, HostId};
use crate::new_session::{Hint, alert};
use crate::{At, Effect, Focus, ReplyTo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialIntent {
    /// `text` is typed into the field `field` of `provider`'s form of
    /// `kind`.
    Type {
        provider: Provider,
        kind: String,
        field: String,
        text: String,
    },
    Save {
        provider: Provider,
        kind: String,
    },
    StartCredentialSignIn {
        provider: Provider,
        kind: String,
    },
    Remove(InstanceId),
    /// Runs the instance's CLI's own sign-in on the host.
    StartSignIn(InstanceId),
    /// The code typed or pasted from the sign-in page.
    TypeSignInCode(InstanceId, String),
    SubmitSignInCode(InstanceId),
    CancelSignIn(InstanceId),
}

/// Every paired host's instances, a section per host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialsScreen {
    pub hosts: Vec<CredentialsPane>,
}

/// One host's instances.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialsPane {
    pub host: HostId,
    /// The id of the host's heading, which labels its section.
    pub heading: String,
    pub name: String,
    /// The host's link state, also its `data-state`.
    pub state: String,
    /// Forgets the host on this device.
    pub forget: ClearButton,
    pub subtitle: String,
    pub instances: Vec<InstanceCredentials>,
    /// The id of the heading of the credential forms, where the focus goes
    /// once a removed instance's section has gone.
    pub add_heading: String,
    /// The credential forms of each provider the host's plugins describe.
    pub add: Vec<ProviderForms>,
    /// What came of the last removal, whose section has gone.
    pub removed: Option<Hint>,
}

/// One instance: its account, what it signs in with now, its sign-in and
/// its removal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceCredentials {
    pub instance: InstanceId,
    /// The heading's id, which labels the instance's section.
    pub heading: String,
    /// Its account, or its provider's name for the provider's own sign-in.
    pub name: String,
    /// Its provider, as the provider's plugin calls it.
    pub provider: String,
    /// What the instance signs in with now.
    pub state: String,
    /// How to sign its CLI in on the host's terminal.
    pub login: String,
    pub sign_in: SignInView,
    /// `None` for the provider's own sign-in, which stays.
    pub remove: Option<ClearButton>,
    pub outcome: Option<Hint>,
}

/// The instance's sign-in from the app.
///
/// That is the button that starts one, or where the running one stands,
/// with the page to open, the code to enter there, the field for the code
/// the page gives back, and a button that cancels it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInView {
    pub start: Option<SignInButton>,
    pub progress: Option<Hint>,
    /// The sign-in page, which opens in a new tab.
    pub link: Option<String>,
    pub code: Option<DeviceCode>,
    pub paste: Option<PasteForm>,
    pub cancel: Option<ClearButton>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInButton {
    pub text: String,
    /// The button's accessible label.
    pub label: String,
    pub disabled: bool,
}

/// A code the person enters on the sign-in page, and its copy button.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCode {
    pub code: String,
    pub copy_label: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Clipboard {
    Ready,
    Writing,
    Copied,
    Failed,
}

impl Clipboard {
    #[must_use]
    pub const fn finished(copied: Option<bool>) -> Self {
        if matches!(copied, Some(true)) {
            Self::Copied
        } else {
            Self::Failed
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        if matches!(self, Self::Copied) {
            "Copied"
        } else {
            "Copy"
        }
    }

    #[must_use]
    pub const fn disabled(self) -> bool {
        matches!(self, Self::Writing)
    }

    #[must_use]
    pub const fn error(self) -> Option<&'static str> {
        if matches!(self, Self::Failed) {
            Some("Could not copy the code. Select and copy it manually.")
        } else {
            None
        }
    }
}

/// The field for the code the sign-in page gives, a password field so the
/// screen doesn't show it, and its send button.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasteForm {
    pub instance: InstanceId,
    /// The form's accessible label.
    pub label: String,
    pub value: String,
    pub submit_disabled: bool,
}

/// A provider's credential forms. A credential is of the account its form
/// names: the instance of that account takes it, or comes to be with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderForms {
    pub provider: Provider,
    /// The provider, as its plugin calls it.
    pub label: String,
    pub forms: Vec<KindForm>,
    pub outcome: Option<Hint>,
}

/// The form of one kind of credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KindForm {
    pub provider: Provider,
    pub kind: String,
    pub label: String,
    pub fields: Vec<FieldView>,
    pub save_label: String,
    pub save_disabled: bool,
    pub setup_sign_in: Option<SignInButton>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClearButton {
    pub label: String,
    pub disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldView {
    pub provider: Provider,
    pub kind: String,
    pub field: String,
    pub label: String,
    /// The input's `type`.
    pub input: &'static str,
    pub placeholder: String,
    /// A password manager ignores `off` on a password field, so a secret
    /// says it is new and isn't filled in from a saved one.
    pub autocomplete: &'static str,
    pub value: String,
}

/// What an instance command is about: an instance, or a provider's
/// credential, whose account makes its instance.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Subject {
    Instance(InstanceId),
    Add(Provider),
}

/// What came of the last thing a subject's forms sent.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Sent {
    Sending,
    Done(Done),
    Failed(String),
}

/// What an instance command did once the host took it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Done {
    Stored,
    Removed,
    /// A sign-in command, whose progress the host reports live.
    SignIn,
}

/// What each form's fields hold, and what each subject's forms sent last.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Forms {
    drafts: BTreeMap<(Provider, String), BTreeMap<String, String>>,
    sent: BTreeMap<Subject, Sent>,
    /// The code typed for each instance's sign-in, until it is sent.
    codes: BTreeMap<InstanceId, String>,
}

impl Forms {
    pub fn update(
        mut self,
        intent: CredentialIntent,
        at: At,
        catalog: &Catalog,
    ) -> (Self, Vec<Effect>) {
        match intent {
            CredentialIntent::Type {
                provider,
                kind,
                field,
                text,
            } => {
                self.drafts
                    .entry((provider, kind))
                    .or_default()
                    .insert(field, text);
                (self, Vec::new())
            }
            CredentialIntent::Save { provider, kind } => {
                let described = catalog
                    .of(provider)
                    .and_then(|info| info.form(&kind))
                    .cloned();
                match described {
                    Some(form) => self.save(provider, &form, at),
                    None => (self, Vec::new()),
                }
            }
            CredentialIntent::StartCredentialSignIn { provider, kind } => {
                self.setup_sign_in(provider, kind, catalog, at)
            }
            CredentialIntent::Remove(instance) => {
                let body = CommandBody::RemoveInstance {
                    instance: instance.clone(),
                };
                let mut effects = self.send(
                    Subject::Instance(instance),
                    Done::Removed,
                    body,
                    at,
                );
                effects
                    .push(Effect::Focus(Focus::Element(add_heading(at.host))));
                (self, effects)
            }
            signing @ (CredentialIntent::StartSignIn(_)
            | CredentialIntent::TypeSignInCode(..)
            | CredentialIntent::SubmitSignInCode(_)
            | CredentialIntent::CancelSignIn(_)) => self.signing(signing, at),
        }
    }

    /// What a sign-in's controls do.
    fn signing(
        mut self,
        intent: CredentialIntent,
        at: At,
    ) -> (Self, Vec<Effect>) {
        match intent {
            CredentialIntent::StartSignIn(instance) => {
                self.forget_sign_in(&instance);
                let body = CommandBody::StartSignIn {
                    instance: instance.clone(),
                };
                let effects = self.sign_in(instance, body, at);
                (self, effects)
            }
            CredentialIntent::TypeSignInCode(instance, text) => {
                self.codes.insert(instance, text);
                (self, Vec::new())
            }
            CredentialIntent::SubmitSignInCode(instance) => {
                let typed = self.codes.remove(&instance).unwrap_or_default();
                let body = CommandBody::SubmitSignInCode {
                    instance: instance.clone(),
                    code: SignInCode(String::from(typed.trim())),
                };
                let effects = self.sign_in(instance, body, at);
                (self, effects)
            }
            CredentialIntent::CancelSignIn(instance) => {
                self.forget_sign_in(&instance);
                let body = CommandBody::CancelSignIn {
                    instance: instance.clone(),
                };
                let effects = self.sign_in(instance, body, at);
                (self, effects)
            }
            CredentialIntent::Type { .. }
            | CredentialIntent::Save { .. }
            | CredentialIntent::StartCredentialSignIn { .. }
            | CredentialIntent::Remove(_) => (self, Vec::new()),
        }
    }

    fn setup_sign_in(
        mut self,
        provider: Provider,
        kind: String,
        catalog: &Catalog,
        at: At,
    ) -> (Self, Vec<Effect>) {
        let requested_label = catalog
            .of(provider)
            .and_then(|info| info.form(&kind))
            .filter(|form| form.setup_sign_in.is_some())
            .and_then(|form| {
                self.drafts
                    .get(&(provider, kind.clone()))
                    .and_then(|draft| setup_label(form, draft))
            });
        let Some(label) = requested_label else {
            return (self, Vec::new());
        };
        let instance = InstanceId::own(provider);
        let target = heading(at.host, &instance);
        let body = CommandBody::StartCredentialSignIn {
            instance: instance.clone(),
            kind,
            label,
        };
        self.forget_sign_in(&instance);
        let mut effects = self.sign_in(instance, body, at);
        effects.push(Effect::Focus(Focus::Element(target)));
        (self, effects)
    }

    fn sign_in(
        &mut self,
        instance: InstanceId,
        body: CommandBody,
        at: At,
    ) -> Vec<Effect> {
        self.send(Subject::Instance(instance), Done::SignIn, body, at)
    }

    /// Drops what the person typed for the instance's last
    /// sign-in.
    fn forget_sign_in(&mut self, instance: &InstanceId) {
        self.codes.remove(instance);
    }

    /// Sends `body` for `subject`, which shows it is sending until the
    /// answer.
    fn send(
        &mut self,
        subject: Subject,
        done: Done,
        body: CommandBody,
        at: At,
    ) -> Vec<Effect> {
        self.sent.insert(subject.clone(), Sent::Sending);
        Vec::from([at.send(body, ReplyTo::Instance(subject, done))])
    }

    /// Sends the form's values, the text ones trimmed, and empties its
    /// secrets.
    fn save(
        mut self,
        provider: Provider,
        form: &CredentialForm,
        at: At,
    ) -> (Self, Vec<Effect>) {
        let key = (provider, form.kind.clone());
        let draft = self.drafts.remove(&key).unwrap_or_default();
        let values = form
            .fields
            .iter()
            .map(|field| (field.id.clone(), sent_value(field, &draft)))
            .collect::<BTreeMap<String, String>>();
        let kept = draft
            .into_iter()
            .filter(|(id, _)| !secret(form, id))
            .collect::<BTreeMap<String, String>>();
        self.drafts.insert(key, kept);
        let credential = Credential {
            kind: form.kind.clone(),
            values,
        };
        let body = CommandBody::SetCredential {
            provider,
            credential,
        };
        let effects = self.send(Subject::Add(provider), Done::Stored, body, at);
        (self, effects)
    }

    /// The host's answer to what `subject`'s forms sent.
    pub fn replied(
        mut self,
        subject: Subject,
        done: Done,
        answer: Result<CommandResult, String>,
        host: &str,
    ) -> Self {
        let sent = match answer {
            Ok(CommandResult::Instances(_)) => Sent::Done(done),
            Ok(CommandResult::Accepted) if done == Done::SignIn => {
                Sent::Done(done)
            }
            Ok(CommandResult::Rejected(rejection)) => {
                Sent::Failed(rejection.label())
            }
            Ok(
                CommandResult::Accepted
                | CommandResult::Started { .. }
                | CommandResult::Diff(_)
                | CommandResult::IssueCreated { .. }
                | CommandResult::PushPublicKey(_)
                | CommandResult::PushUnavailable
                | CommandResult::FleetStatus(_)
                | CommandResult::FleetInvite(_)
                | CommandResult::FleetPairing(_),
            ) => Sent::Failed(format!("{host} gave an unexpected answer.")),
            Err(error) => Sent::Failed(error),
        };
        self.sent.insert(subject, sent);
        self
    }

    /// A section for each instance the host has, with where its sign-in
    /// from the app stands, and the credential forms of each provider its
    /// plugins describe.
    pub fn screen(&self, id: HostId, known: &Host) -> CredentialsPane {
        let name = known.name();
        let host = name.as_str();
        let instances = known.fold.instances.as_ref();
        let catalog = &known.fold.catalog;
        let sign_ins = &known.fold.sign_ins;
        let listed = instances.map_or_else(Vec::new, |listed| {
            listed
                .0
                .iter()
                .map(|(instance, status)| {
                    self.instance(
                        (id, instance),
                        status,
                        catalog,
                        (host, sign_ins.get(instance)),
                    )
                })
                .collect()
        });
        CredentialsPane {
            host: id,
            heading: format!("host-{}", id.key()),
            state: known.status_line().state,
            forget: ClearButton {
                label: format!("Forget {host}"),
                disabled: false,
            },
            add_heading: add_heading(id),
            subtitle: format!(
                "How {host} reaches each agent's model. Each instance is one account, with its own credential and its own sign-in."
            ),
            instances: listed,
            add: catalog
                .0
                .iter()
                .map(|(&provider, info)| {
                    let own = InstanceId::own(provider);
                    let busy = self.sent.get(&Subject::Instance(own.clone()))
                        == Some(&Sent::Sending)
                        || matches!(
                            sign_ins.get(&own),
                            Some(
                                SignInStep::Starting
                                    | SignInStep::Open { .. }
                                    | SignInStep::Finishing
                            )
                        );
                    self.forms(provider, info, host, busy)
                })
                .collect(),
            removed: self.removed(instances, host),
            name,
        }
    }

    /// The instance's section, on `host` where its sign-in from the app
    /// stands at `step`.
    fn instance(
        &self,
        (id, instance): (HostId, &InstanceId),
        status: &InstanceStatus,
        catalog: &Catalog,
        (host, step): (&str, Option<&SignInStep>),
    ) -> InstanceCredentials {
        let sent = self.sent.get(&Subject::Instance(instance.clone()));
        let sending = sent == Some(&Sent::Sending);
        let info = catalog.of(instance.provider);
        let name = instance_name(instance, catalog);
        let provider = instance.provider.name();
        InstanceCredentials {
            instance: instance.clone(),
            heading: heading(id, instance),
            provider: catalog.label(instance.provider),
            state: describe(info, &status.availability, host),
            login: match &instance.account {
                Account::Own => format!(
                    "To sign in on {host}'s terminal: conductord login {provider}"
                ),
                Account::Named(account) => format!(
                    "To sign it in again on {host}'s terminal: conductord login {provider} --account={}",
                    shell_argument(account),
                ),
            },
            sign_in: self.sign_in_view(instance, &name, (step, sending), host),
            remove: (instance.account != Account::Own).then(|| ClearButton {
                label: format!("Remove {name}"),
                disabled: sending,
            }),
            outcome: sent.and_then(|last| outcome(last, host)),
            name,
        }
    }

    /// A provider's credential forms, and what came of the last it sent.
    fn forms(
        &self,
        provider: Provider,
        info: &ProviderInfo,
        host: &str,
        busy: bool,
    ) -> ProviderForms {
        let sent = self.sent.get(&Subject::Add(provider));
        ProviderForms {
            provider,
            label: info.label.clone(),
            forms: info
                .credentials
                .iter()
                .map(|form| self.form(provider, form, sent, busy))
                .collect(),
            outcome: sent.and_then(|last| outcome(last, host)),
        }
    }

    /// Says a removal took once its instance has gone from the list.
    fn removed(
        &self,
        instances: Option<&Instances>,
        host: &str,
    ) -> Option<Hint> {
        self.sent
            .iter()
            .any(|(subject, sent)| match (subject, sent) {
                (Subject::Instance(id), Sent::Done(Done::Removed)) => {
                    instances.is_some_and(|known| known.of(id).is_none())
                }
                _ => false,
            })
            .then(|| status(format!("Removed from {host}.")))
    }

    fn form(
        &self,
        provider: Provider,
        form: &CredentialForm,
        sent: Option<&Sent>,
        busy: bool,
    ) -> KindForm {
        let empty = BTreeMap::new();
        let draft = self
            .drafts
            .get(&(provider, form.kind.clone()))
            .unwrap_or(&empty);
        KindForm {
            provider,
            kind: form.kind.clone(),
            label: form.label.clone(),
            fields: form
                .fields
                .iter()
                .map(|field| FieldView {
                    provider,
                    kind: form.kind.clone(),
                    field: field.id.clone(),
                    label: field.label.clone(),
                    input: input(field.kind),
                    placeholder: field.placeholder.clone(),
                    autocomplete: autocomplete(field.kind),
                    value: draft.get(&field.id).cloned().unwrap_or_default(),
                })
                .collect(),
            setup_sign_in: form.setup_sign_in.as_ref().map(|text| {
                SignInButton {
                    text: text.clone(),
                    label: format!("{text}: {}", form.label),
                    disabled: busy || setup_label(form, draft).is_none(),
                }
            }),
            save_label: format!("Save {}", form.label.to_lowercase()),
            save_disabled: sent == Some(&Sent::Sending)
                || form.fields.iter().any(|field| {
                    field.required && sent_value(field, draft).is_empty()
                }),
        }
    }
}

impl Forms {
    /// The instance's sign-in, at `step`, while a command for the
    /// instance may be `sending`.
    fn sign_in_view(
        &self,
        instance: &InstanceId,
        name: &str,
        (step, sending): (Option<&SignInStep>, bool),
        host: &str,
    ) -> SignInView {
        let cancel = Some(ClearButton {
            label: format!("Cancel the sign-in of {name}"),
            disabled: sending,
        });
        let start = |text: &str| SignInButton {
            text: String::from(text),
            label: format!("{text}: {name}"),
            disabled: sending,
        };
        let idle = SignInView {
            start: Some(start("Sign in")),
            progress: None,
            link: None,
            code: None,
            paste: None,
            cancel: None,
        };
        match step {
            None | Some(SignInStep::Cancelled) => idle,
            Some(SignInStep::Starting) => SignInView {
                start: None,
                progress: Some(status(format!(
                    "Starting the sign-in on {host}…"
                ))),
                cancel,
                ..idle
            },
            Some(SignInStep::Open { url, code, paste }) => SignInView {
                start: None,
                progress: Some(status(String::from(open_says(
                    code.is_some(),
                    *paste,
                )))),
                link: Some(url.clone()),
                code: code.as_ref().map(|shown| device_code(shown)),
                paste: paste.then(|| self.paste_form(instance, name, sending)),
                cancel,
            },
            Some(SignInStep::Finishing) => SignInView {
                start: None,
                progress: Some(status(format!(
                    "Finishing the sign-in on {host}…"
                ))),
                cancel,
                ..idle
            },
            Some(SignInStep::SignedIn { .. }) => SignInView {
                start: Some(start("Sign in again")),
                progress: Some(status(format!("Signed in on {host}."))),
                ..idle
            },
            Some(SignInStep::Failed { reason }) => SignInView {
                progress: Some(alert(format!("The sign-in failed: {reason}"))),
                ..idle
            },
        }
    }

    fn paste_form(
        &self,
        instance: &InstanceId,
        name: &str,
        sending: bool,
    ) -> PasteForm {
        let value = self.codes.get(instance).cloned().unwrap_or_default();
        PasteForm {
            instance: instance.clone(),
            label: format!("Code for {name}"),
            submit_disabled: sending || value.trim().is_empty(),
            value,
        }
    }
}

fn shell_argument(argument: &str) -> String {
    format!("'{}'", argument.replace('\'', "'\\''"))
}

fn device_code(code: &str) -> DeviceCode {
    DeviceCode {
        code: String::from(code),
        copy_label: format!("Copy the code {code}"),
    }
}

/// What a person does with a sign-in page that shows a `code` to enter,
/// or gives one to `paste`, or neither.
const fn open_says(code: bool, paste: bool) -> &'static str {
    match (code, paste) {
        (true, _) => {
            "Open the sign-in page, sign in, and enter this code there. This page follows along."
        }
        (false, true) => {
            "Copy the code from the sign-in page. Return to Conductor on this device and paste it here."
        }
        (false, false) => {
            "Open the sign-in page and sign in there. This page follows along."
        }
    }
}

fn setup_label(
    form: &CredentialForm,
    draft: &BTreeMap<String, String>,
) -> Option<String> {
    let mut fields =
        form.fields.iter().filter(|field| field.id == form.account);
    let field = fields.next()?;
    if field.kind != FieldKind::Text || fields.next().is_some() {
        return None;
    }
    let label = sent_value(field, draft);
    (!label.is_empty()).then_some(label)
}

/// What a field sends: a secret as typed, anything else trimmed.
fn sent_value(field: &Field, draft: &BTreeMap<String, String>) -> String {
    let typed = draft.get(&field.id).map_or("", String::as_str);
    match field.kind {
        FieldKind::Secret => String::from(typed),
        FieldKind::Text | FieldKind::Url => String::from(typed.trim()),
    }
}

fn secret(form: &CredentialForm, id: &str) -> bool {
    form.fields
        .iter()
        .any(|field| field.id == id && field.kind == FieldKind::Secret)
}

const fn input(kind: FieldKind) -> &'static str {
    match kind {
        FieldKind::Text => "text",
        FieldKind::Url => "url",
        FieldKind::Secret => "password",
    }
}

const fn autocomplete(kind: FieldKind) -> &'static str {
    match kind {
        FieldKind::Secret => "new-password",
        FieldKind::Text | FieldKind::Url => "off",
    }
}

/// What the app calls an instance: its account, or for its provider's own
/// sign-in the provider's label.
#[must_use]
pub fn instance_name(instance: &InstanceId, catalog: &Catalog) -> String {
    match &instance.account {
        Account::Own => catalog.label(instance.provider),
        Account::Named(account) => account.clone(),
    }
}

/// An element id for the instance's heading: the account with no
/// whitespace, which an id can't hold.
fn heading(host: HostId, instance: &InstanceId) -> String {
    let account = match &instance.account {
        Account::Own => String::from("own"),
        Account::Named(account) => account
            .chars()
            .map(|c| if c.is_whitespace() { '-' } else { c })
            .collect(),
    };
    format!(
        "{}-{}-{account}-credential",
        host.key(),
        instance.provider.name()
    )
}

fn add_heading(host: HostId) -> String {
    format!("{}-add-instance", host.key())
}

const fn status(text: String) -> Hint {
    Hint {
        class: "hint muted",
        role: Some("status"),
        text,
    }
}

fn outcome(sent: &Sent, host: &str) -> Option<Hint> {
    match sent {
        Sent::Sending => Some(status(String::from("Sending…"))),
        Sent::Done(Done::Stored) => Some(status(format!(
            "Sent to {host} over the encrypted link. This app doesn't keep it; your browser may offer to save it."
        ))),
        Sent::Done(Done::Removed | Done::SignIn) => None,
        Sent::Failed(error) => Some(alert(error.clone())),
    }
}

/// What an instance signs in with, in words. Never more than a stored
/// credential's kind and URL.
fn describe(
    info: Option<&ProviderInfo>,
    availability: &Availability,
    host: &str,
) -> String {
    match availability {
        Availability::Available {
            auth: Auth::Stored { kind, url },
            ..
        } => {
            let label = info
                .and_then(|known| known.form(kind))
                .map_or_else(|| kind.clone(), |form| form.label.clone());
            url.as_ref()
                .map_or_else(|| label.clone(), |at| format!("{label} at {at}"))
        }
        Availability::Available {
            auth: Auth::SignedIn,
            ..
        } => format!("Signed in on {host}"),
        Availability::Available {
            auth: Auth::Unmanaged,
            ..
        } => format!("{host}'s own CLI, signed in on its own"),
        Availability::Unavailable { reason } => {
            format!("Not available: {reason}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::shell_argument;

    #[test]
    fn terminal_account_is_one_literal_shell_argument() {
        assert_eq!(shell_argument("two words"), "'two words'");
        assert_eq!(shell_argument("O'Neil"), "'O'\\''Neil'");
    }
}
