//! The host's instances and their credentials: what each instance signs in
//! with, its CLI's own sign-in run from the app, a form for each kind of
//! credential its provider's plugin describes, a button to clear the
//! stored one, its label to rename it, a button to remove it, and a form
//! per provider to add another instance of it. A secret only passes
//! through: its field is cleared once it is sent, and nothing keeps it
//! (§6.3). A pasted sign-in code is one.

use alloc::collections::{BTreeMap, BTreeSet};

use crate::prelude::*;
use proto::{
    Auth, Availability, Catalog, CommandBody, CommandResult, Credential,
    CredentialForm, Field, FieldKind, InstanceId, InstanceStatus, Instances,
    Provider, ProviderInfo, SignInCode, SignInStep,
};

use crate::new_session::{Hint, alert};
use crate::{Effect, Focus, Reply, ReplyTo};

/// The id of the heading of the forms that add instances, where the focus
/// goes once a removed instance's section has gone.
pub const ADD_HEADING: &str = "add-instance";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialIntent {
    /// `text` is typed into the field `field` of `instance`'s form of
    /// `kind`.
    Type {
        instance: InstanceId,
        kind: String,
        field: String,
        text: String,
    },
    Save {
        instance: InstanceId,
        kind: String,
    },
    Clear(InstanceId),
    /// The label typed for an instance's new name.
    TypeLabel(InstanceId, String),
    Rename(InstanceId),
    Remove(InstanceId),
    /// The label typed for a new instance of a provider.
    TypeNewLabel(Provider, String),
    Add(Provider),
    /// Runs the instance's CLI's own sign-in on the host.
    StartSignIn(InstanceId),
    /// The code typed or pasted from the sign-in page.
    TypeSignInCode(InstanceId, String),
    SubmitSignInCode(InstanceId),
    CancelSignIn(InstanceId),
    /// Copies the code the sign-in shows to the clipboard.
    CopySignInCode(InstanceId, String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialsPane {
    pub subtitle: String,
    pub instances: Vec<InstanceCredentials>,
    /// A form per provider the host's plugins describe.
    pub add: Vec<AddForm>,
    /// What came of the last removal, whose section has gone.
    pub removed: Option<Hint>,
}

/// One instance: what it signs in with now, a form for each kind of
/// credential its provider takes, and its label and removal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceCredentials {
    pub instance: InstanceId,
    /// The heading's id, which labels the instance's section and takes
    /// the focus when the clear button goes.
    pub heading: String,
    pub name: String,
    /// Its provider, as the provider's plugin calls it.
    pub provider: String,
    /// What the instance signs in with now.
    pub state: String,
    /// How to sign its CLI in on the host's terminal.
    pub login: String,
    pub sign_in: SignInView,
    pub clear: Option<ClearButton>,
    pub rename: RenameForm,
    pub remove: ClearButton,
    pub forms: Vec<KindForm>,
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
    /// The copy button's text: whether it has copied the code.
    pub copy: String,
    pub copy_label: String,
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

/// The instance's label, and the button that renames it to the typed one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameForm {
    pub value: String,
    /// The field's accessible label.
    pub label: String,
    pub disabled: bool,
}

/// A label for another instance of `provider`, and the button that adds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddForm {
    pub provider: Provider,
    pub value: String,
    pub placeholder: String,
    pub button: String,
    pub disabled: bool,
    pub outcome: Option<Hint>,
}

/// The form of one kind of credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KindForm {
    pub instance: InstanceId,
    pub kind: String,
    pub label: String,
    pub fields: Vec<FieldView>,
    pub save_label: String,
    pub save_disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClearButton {
    pub label: String,
    pub disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldView {
    pub instance: InstanceId,
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

/// What an instance command is about: an instance, or a new instance of a
/// provider.
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
    Cleared,
    Renamed,
    Removed,
    Added,
    /// A sign-in command, whose progress the host reports live.
    SignIn,
}

/// What each form's fields hold, and what each subject's forms sent last.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Forms {
    drafts: BTreeMap<(InstanceId, String), BTreeMap<String, String>>,
    labels: BTreeMap<InstanceId, String>,
    new_labels: BTreeMap<Provider, String>,
    sent: BTreeMap<Subject, Sent>,
    /// The code typed for each instance's sign-in, until it is sent.
    codes: BTreeMap<InstanceId, String>,
    /// The instances whose shown code was copied.
    copied: BTreeSet<InstanceId>,
}

impl Forms {
    pub fn update(
        mut self,
        intent: CredentialIntent,
        visit: u64,
        known: (Option<&Instances>, &Catalog),
    ) -> (Self, Vec<Effect>) {
        let (instances, catalog) = known;
        match intent {
            CredentialIntent::Type {
                instance,
                kind,
                field,
                text,
            } => {
                self.drafts
                    .entry((instance, kind))
                    .or_default()
                    .insert(field, text);
                (self, Vec::new())
            }
            CredentialIntent::Save { instance, kind } => {
                let described = instances
                    .and_then(|known| known.of(&instance))
                    .and_then(|status| catalog.of(status.provider))
                    .and_then(|info| info.form(&kind))
                    .cloned();
                match described {
                    Some(form) => self.save(instance, &form, visit),
                    None => (self, Vec::new()),
                }
            }
            CredentialIntent::Clear(instance) => {
                let focus = Focus::Element(heading(&instance));
                let body = CommandBody::ClearCredential {
                    instance: instance.clone(),
                };
                let mut effects = self.send(
                    Subject::Instance(instance),
                    Done::Cleared,
                    body,
                    visit,
                );
                // The button goes once the clear lands, so focus goes to
                // the heading instead of the page.
                effects.push(Effect::Focus(focus));
                (self, effects)
            }
            CredentialIntent::TypeLabel(instance, text) => {
                self.labels.insert(instance, text);
                (self, Vec::new())
            }
            CredentialIntent::Rename(instance) => {
                let effects = self.rename(instance, visit);
                (self, effects)
            }
            CredentialIntent::Remove(instance) => {
                let body = CommandBody::RemoveInstance {
                    instance: instance.clone(),
                };
                let mut effects = self.send(
                    Subject::Instance(instance),
                    Done::Removed,
                    body,
                    visit,
                );
                effects.push(Effect::Focus(Focus::Element(String::from(
                    ADD_HEADING,
                ))));
                (self, effects)
            }
            CredentialIntent::TypeNewLabel(provider, text) => {
                self.new_labels.insert(provider, text);
                (self, Vec::new())
            }
            CredentialIntent::Add(provider) => {
                let label = self
                    .new_labels
                    .get(&provider)
                    .map_or_else(String::new, |typed| {
                        String::from(typed.trim())
                    });
                let body = CommandBody::AddInstance { provider, label };
                let effects =
                    self.send(Subject::Add(provider), Done::Added, body, visit);
                (self, effects)
            }
            signing @ (CredentialIntent::StartSignIn(_)
            | CredentialIntent::TypeSignInCode(..)
            | CredentialIntent::SubmitSignInCode(_)
            | CredentialIntent::CancelSignIn(_)
            | CredentialIntent::CopySignInCode(..)) => {
                self.signing(signing, visit)
            }
        }
    }

    /// Renames the instance to the label typed for it.
    fn rename(&mut self, instance: InstanceId, visit: u64) -> Vec<Effect> {
        let label = self
            .labels
            .get(&instance)
            .map_or_else(String::new, |typed| String::from(typed.trim()));
        let body = CommandBody::RenameInstance {
            instance: instance.clone(),
            label,
        };
        self.send(Subject::Instance(instance), Done::Renamed, body, visit)
    }

    /// What a sign-in's controls do.
    fn signing(
        mut self,
        intent: CredentialIntent,
        visit: u64,
    ) -> (Self, Vec<Effect>) {
        match intent {
            CredentialIntent::StartSignIn(instance) => {
                self.forget_sign_in(&instance);
                let body = CommandBody::StartSignIn {
                    instance: instance.clone(),
                };
                let effects = self.sign_in(instance, body, visit);
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
                let effects = self.sign_in(instance, body, visit);
                (self, effects)
            }
            CredentialIntent::CancelSignIn(instance) => {
                self.forget_sign_in(&instance);
                let body = CommandBody::CancelSignIn {
                    instance: instance.clone(),
                };
                let effects = self.sign_in(instance, body, visit);
                (self, effects)
            }
            CredentialIntent::CopySignInCode(instance, code) => {
                self.copied.insert(instance);
                (self, Vec::from([Effect::Copy(code)]))
            }
            CredentialIntent::Type { .. }
            | CredentialIntent::Save { .. }
            | CredentialIntent::Clear(_)
            | CredentialIntent::TypeLabel(..)
            | CredentialIntent::Rename(_)
            | CredentialIntent::Remove(_)
            | CredentialIntent::TypeNewLabel(..)
            | CredentialIntent::Add(_) => (self, Vec::new()),
        }
    }

    fn sign_in(
        &mut self,
        instance: InstanceId,
        body: CommandBody,
        visit: u64,
    ) -> Vec<Effect> {
        self.send(Subject::Instance(instance), Done::SignIn, body, visit)
    }

    /// Drops what the person typed or copied for the instance's last
    /// sign-in.
    fn forget_sign_in(&mut self, instance: &InstanceId) {
        self.codes.remove(instance);
        self.copied.remove(instance);
    }

    /// Sends `body` for `subject`, which shows it is sending until the
    /// answer.
    fn send(
        &mut self,
        subject: Subject,
        done: Done,
        body: CommandBody,
        visit: u64,
    ) -> Vec<Effect> {
        self.sent.insert(subject.clone(), Sent::Sending);
        Vec::from([Effect::Send {
            body,
            reply: Reply {
                visit,
                to: ReplyTo::Instance(subject, done),
            },
        }])
    }

    /// Sends the form's values, the text ones trimmed, and empties its
    /// secrets.
    fn save(
        mut self,
        instance: InstanceId,
        form: &CredentialForm,
        visit: u64,
    ) -> (Self, Vec<Effect>) {
        let key = (instance.clone(), form.kind.clone());
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
            instance: instance.clone(),
            credential,
        };
        let effects =
            self.send(Subject::Instance(instance), Done::Stored, body, visit);
        (self, effects)
    }

    /// The host's answer to what `subject`'s forms sent. A rename or an add
    /// that took empties its field.
    pub fn replied(
        mut self,
        subject: Subject,
        done: Done,
        answer: Result<CommandResult, String>,
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
                | CommandResult::Diff(_),
            ) => Sent::Failed(String::from(
                "The host gave an unexpected answer.",
            )),
            Err(error) => Sent::Failed(error),
        };
        match (&subject, &sent) {
            (Subject::Instance(instance), Sent::Done(Done::Renamed)) => {
                self.labels.remove(instance);
            }
            (Subject::Add(provider), Sent::Done(Done::Added)) => {
                self.new_labels.remove(provider);
            }
            _ => {}
        }
        self.sent.insert(subject, sent);
        self
    }

    /// A section for each instance the host has, with where its sign-in
    /// from the app stands, and a form for adding one of each provider its
    /// plugins describe.
    pub fn screen(
        &self,
        host: &str,
        instances: Option<&Instances>,
        catalog: &Catalog,
        sign_ins: &BTreeMap<InstanceId, SignInStep>,
    ) -> CredentialsPane {
        let listed = instances.map_or_else(Vec::new, |known| {
            known
                .0
                .iter()
                .map(|(id, status)| {
                    self.instance(id, status, catalog, (host, sign_ins.get(id)))
                })
                .collect()
        });
        CredentialsPane {
            subtitle: format!(
                "How {host} reaches each agent's model. Each instance has its own credential and its own sign-in."
            ),
            instances: listed,
            add: catalog
                .0
                .iter()
                .map(|(&provider, info)| self.add_form(provider, info, host))
                .collect(),
            removed: self.removed(instances, host),
        }
    }

    /// The instance's section, on `host` where its sign-in from the app
    /// stands at `step`.
    fn instance(
        &self,
        instance: &InstanceId,
        status: &InstanceStatus,
        catalog: &Catalog,
        (host, step): (&str, Option<&SignInStep>),
    ) -> InstanceCredentials {
        let sent = self.sent.get(&Subject::Instance(instance.clone()));
        let sending = sent == Some(&Sent::Sending);
        let info = catalog.of(status.provider);
        let typed = self.labels.get(instance);
        let value = typed.cloned().unwrap_or_else(|| status.label.clone());
        let renamed = value.trim();
        InstanceCredentials {
            instance: instance.clone(),
            heading: heading(instance),
            name: status.label.clone(),
            provider: catalog.label(status.provider),
            state: describe(info, &status.availability),
            login: format!(
                "To sign it in on {host}: conductord login {}",
                instance.0
            ),
            sign_in: self.sign_in_view(
                instance,
                &status.label,
                (step, sending),
                host,
            ),
            clear: stored(&status.availability).then(|| ClearButton {
                label: format!("Clear the {} credential", status.label),
                disabled: sending,
            }),
            rename: RenameForm {
                label: format!("Label of {}", status.label),
                disabled: sending
                    || renamed.is_empty()
                    || renamed == status.label,
                value,
            },
            remove: ClearButton {
                label: format!("Remove {}", status.label),
                disabled: sending,
            },
            forms: info.map_or_else(Vec::new, |described| {
                described
                    .credentials
                    .iter()
                    .map(|form| self.form(instance, form))
                    .collect()
            }),
            outcome: sent.and_then(|last| outcome(last, host)),
        }
    }

    fn add_form(
        &self,
        provider: Provider,
        info: &ProviderInfo,
        host: &str,
    ) -> AddForm {
        let sent = self.sent.get(&Subject::Add(provider));
        let value = self.new_labels.get(&provider).cloned().unwrap_or_default();
        AddForm {
            provider,
            placeholder: format!("{} (API)", info.label),
            button: format!("Add instance: {}", info.label),
            disabled: value.trim().is_empty() || sent == Some(&Sent::Sending),
            value,
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

    fn form(&self, instance: &InstanceId, form: &CredentialForm) -> KindForm {
        let empty = BTreeMap::new();
        let draft = self
            .drafts
            .get(&(instance.clone(), form.kind.clone()))
            .unwrap_or(&empty);
        KindForm {
            instance: instance.clone(),
            kind: form.kind.clone(),
            label: form.label.clone(),
            fields: form
                .fields
                .iter()
                .map(|field| FieldView {
                    instance: instance.clone(),
                    kind: form.kind.clone(),
                    field: field.id.clone(),
                    label: field.label.clone(),
                    input: input(field.kind),
                    placeholder: field.placeholder.clone(),
                    autocomplete: autocomplete(field.kind),
                    value: draft.get(&field.id).cloned().unwrap_or_default(),
                })
                .collect(),
            save_label: format!("Save {}", form.label.to_lowercase()),
            save_disabled: form.fields.iter().any(|field| {
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
                code: code
                    .as_ref()
                    .map(|shown| self.device_code(instance, shown)),
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

    fn device_code(&self, instance: &InstanceId, code: &str) -> DeviceCode {
        DeviceCode {
            code: String::from(code),
            copy: String::from(if self.copied.contains(instance) {
                "Copied"
            } else {
                "Copy"
            }),
            copy_label: format!("Copy the code {code}"),
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

/// What a person does with a sign-in page that shows a `code` to enter,
/// or gives one to `paste`, or neither.
const fn open_says(code: bool, paste: bool) -> &'static str {
    match (code, paste) {
        (true, _) => {
            "Open the sign-in page, sign in, and enter this code there. This page follows along."
        }
        (false, true) => {
            "Open the sign-in page and sign in. Then paste the code it shows here."
        }
        (false, false) => {
            "Open the sign-in page and sign in there. This page follows along."
        }
    }
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

fn heading(instance: &InstanceId) -> String {
    format!("{}-credential", instance.0)
}

/// Whether the host stores a credential for the instance, which a person
/// may clear.
const fn stored(availability: &Availability) -> bool {
    matches!(
        availability,
        Availability::Available {
            auth: Auth::Stored { .. },
            ..
        }
    )
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
        Sent::Done(Done::Cleared) => {
            Some(status(format!("Cleared on {host}.")))
        }
        Sent::Done(Done::Renamed) => {
            Some(status(format!("Renamed on {host}.")))
        }
        Sent::Done(Done::Added) => Some(status(format!("Added on {host}."))),
        Sent::Done(Done::Removed | Done::SignIn) => None,
        Sent::Failed(error) => Some(alert(error.clone())),
    }
}

/// What an instance signs in with, in words. Never more than a stored
/// credential's kind and URL.
fn describe(
    info: Option<&ProviderInfo>,
    availability: &Availability,
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
        } => String::from("Signed in on the host"),
        Availability::Available {
            auth: Auth::Unmanaged,
            ..
        } => String::from("The host's own CLI, signed in on its own"),
        Availability::Unavailable { reason } => {
            format!("Not available: {reason}")
        }
    }
}
