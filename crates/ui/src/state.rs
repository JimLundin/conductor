//! Every paired host, and what the person has open over them: the place
//! shown, its drafts and forms, and what each intent does to them. What a
//! place holds belongs to the host and session it shows, and goes when
//! another place is shown.

use alloc::collections::BTreeMap;
use core::mem;

use crate::prelude::*;
use client::link::Update;
use proto::{CommandResult, Effort, IssueId, SessionId, Ticket, TicketError};

use crate::credentials::{CredentialsScreen, Forms};
use crate::host::{Host, HostId, by_name};
use crate::issues::{self, IssueDraft, IssueScreen, Shown, Titles};
use crate::new_session::{Form, NewSessionForm};
use crate::projects::{self, ProjectsScreen};
use crate::saved::{Saved, decode};
use crate::session::{Pane, SessionPane};
use crate::sessions::{self, SessionList};
use crate::{
    At, Effect, Intent, IssueIntent, Opening, Pair, Place, Reply, ReplyTo,
    Screen, SessionIntent,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    hosts: BTreeMap<HostId, Host>,
    shown: ShownPlace,
    /// The host of the last session shown, where a new session starts.
    recent: Option<HostId>,
    /// The last command's refusal, shown under any session.
    notice: Option<String>,
    /// Counts the places shown. A form left since it sent a command
    /// ignores the answer.
    visit: u64,
    /// Why the join link the app opened on was refused. The pairing form
    /// says so each time it opens.
    first_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ShownPlace {
    Sessions,
    AddHost(Pairing),
    NewSession(Form),
    Credentials(BTreeMap<HostId, Forms>),
    Projects(Titles),
    Session(HostId, SessionId, Option<Pane>),
    Issue(HostId, IssueId, Option<IssueDraft>),
}

impl ShownPlace {
    const fn place(&self) -> Place {
        match self {
            Self::Sessions => Place::Sessions,
            Self::AddHost(_) => Place::AddHost,
            Self::NewSession(_) => Place::NewSession,
            Self::Credentials(_) => Place::Credentials,
            Self::Projects(_) => Place::Projects,
            Self::Session(host, id, _) => Place::Session(*host, *id),
            Self::Issue(host, id, _) => Place::Issue(*host, *id),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Pairing {
    text: String,
    error: Option<String>,
}

/// The screen once a host is paired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostsScreen {
    /// A link to each paired host, keyed by its ticket, so a host paired
    /// again with another ticket redials.
    pub links: Vec<Dial>,
    /// The pane a phone shows, `list` or `detail`. A wide window shows
    /// both.
    pub shown: &'static str,
    /// The pairing form is shown.
    pub add_current: bool,
    pub credentials_current: bool,
    pub projects_current: bool,
    pub list: SessionList,
    pub detail: Detail,
}

/// A host the shell keeps a link to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dial {
    pub host: HostId,
    pub key: String,
    pub ticket: Ticket,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detail {
    Placeholder(&'static str),
    Pair(Pair),
    NewSession(Box<NewSessionForm>),
    Credentials(Box<CredentialsScreen>),
    Projects(Box<ProjectsScreen>),
    Session(Box<SessionPane>),
    Issue(Box<IssueScreen>),
}

impl State {
    /// The saved hosts, an older app's single ticket among them, and a
    /// host the join link pairs.
    #[must_use]
    pub fn new(opening: Opening) -> Self {
        let first_error = opening
            .joined
            .as_ref()
            .and_then(|joined| joined.as_ref().err())
            .map(invalid);
        let hosts: BTreeMap<_, _> = decode(&opening.stored)
            .into_iter()
            .map(|saved| {
                (
                    HostId::of(&saved.ticket),
                    Host::new(saved.ticket, saved.name),
                )
            })
            .collect();
        let shown = if hosts.is_empty() {
            ShownPlace::AddHost(Pairing::new(first_error.clone()))
        } else {
            ShownPlace::Sessions
        };
        let opened = Self {
            hosts,
            shown,
            recent: None,
            notice: None,
            visit: 0,
            first_error,
        };
        match opening.joined {
            Some(Ok(ticket)) => opened.paired(ticket),
            Some(Err(_)) | None => opened,
        }
    }

    /// The hosts to keep in local storage.
    #[must_use]
    pub fn saved(&self) -> Vec<Saved> {
        self.hosts
            .values()
            .map(|host| Saved {
                ticket: host.ticket.clone(),
                name: host.saved.clone(),
            })
            .collect()
    }

    pub(crate) fn update(mut self, intent: Intent) -> (Self, Vec<Effect>) {
        match intent {
            Intent::TypeJoin(text) => {
                if let ShownPlace::AddHost(pairing) = &mut self.shown {
                    pairing.text = text;
                }
                (self, Vec::new())
            }
            Intent::FleetHosts { hosts, removed } => {
                for host in removed {
                    self = self.forget(host).0;
                }
                for saved in hosts {
                    let id = HostId::of(&saved.ticket);
                    let next = match self.hosts.remove(&id) {
                        Some(known) => Host {
                            ticket: saved.ticket,
                            ..known
                        },
                        None => Host::new(saved.ticket, saved.name),
                    };
                    self.hosts.insert(id, next);
                }
                if !self.hosts.is_empty()
                    && matches!(self.shown, ShownPlace::AddHost(_))
                {
                    self.replace(ShownPlace::Sessions);
                }
                (self, Vec::new())
            }
            Intent::Join(Ok(ticket))
                if matches!(self.shown, ShownPlace::AddHost(_)) =>
            {
                let id = HostId::of(&ticket);
                let saved = Saved {
                    ticket: ticket.clone(),
                    name: self
                        .hosts
                        .get(&id)
                        .and_then(|host| host.saved.clone()),
                };
                (self.paired(ticket), Vec::from([Effect::PairHost(saved)]))
            }
            Intent::Join(Ok(_)) => (self, Vec::new()),
            Intent::Join(Err(error)) => {
                if let ShownPlace::AddHost(pairing) = &mut self.shown {
                    pairing.error = Some(invalid(&error));
                }
                (self, Vec::new())
            }
            Intent::Forget(host) => self.forget(host),
            Intent::Link(host, update) => self.linked(host, update),
            Intent::Show(place) => (self.show(place), Vec::new()),
            Intent::DismissNotice => {
                self.notice = None;
                (self, Vec::new())
            }
            Intent::Form(change) => self.form(change),
            Intent::Credential(host, change) => self.credential(host, change),
            Intent::Project(host, change) => {
                let effects = if matches!(self.shown, ShownPlace::Projects(_))
                    && self.hosts.contains_key(&host)
                {
                    change.send(At {
                        host,
                        visit: self.visit,
                    })
                } else {
                    Vec::new()
                };
                (self, effects)
            }
            Intent::Session(change) => self.session(change),
            Intent::NewIssue(change) => self.new_issue(change),
            Intent::Issue(change) => self.issue(change),
            Intent::StartOn(host, issue) => {
                if !self.hosts.is_empty() {
                    self.replace(ShownPlace::NewSession(Form::on_issue(
                        host, issue,
                    )));
                }
                (self, Vec::new())
            }
            Intent::Replied(reply, answer) => self.replied(reply, answer),
        }
    }

    fn paired(mut self, ticket: Ticket) -> Self {
        let id = HostId::of(&ticket);
        let host = match self.hosts.remove(&id) {
            Some(known) => Host { ticket, ..known },
            None => Host::new(ticket, None),
        };
        self.hosts.insert(id, host);
        if matches!(self.shown, ShownPlace::AddHost(_)) {
            self.replace(ShownPlace::Sessions);
        }
        self
    }

    fn forget(mut self, host: HostId) -> (Self, Vec<Effect>) {
        self.hosts.remove(&host);
        self.recent = self.recent.filter(|recent| *recent != host);
        if self.hosts.is_empty() || matches!(self.shown, ShownPlace::AddHost(_))
        {
            self.replace(ShownPlace::AddHost(Pairing::new(
                self.first_error.clone(),
            )));
        } else if self.shown.place().host() == Some(host) {
            self.replace(ShownPlace::Sessions);
        } else {
            match &mut self.shown {
                ShownPlace::Credentials(forms) => {
                    forms.remove(&host);
                }
                ShownPlace::NewSession(form) => {
                    if form.cancel_start_on(host) {
                        self.visit = self.visit.wrapping_add(1);
                    }
                }
                _ => {}
            }
        }
        (self, Vec::from([Effect::ForgetHost(host)]))
    }

    fn linked(mut self, host: HostId, update: Update) -> (Self, Vec<Effect>) {
        let Some(known) = self.hosts.remove(&host) else {
            return (self, Vec::new());
        };
        let (next, renamed) = known.linked(update);
        let left = match &mut self.shown {
            ShownPlace::Session(selected, id, draft) if *selected == host => {
                sync_draft(draft, next.fold.view.sessions.contains_key(id))
            }
            ShownPlace::Issue(selected, id, draft) if *selected == host => {
                sync_draft(draft, issues::find(&next, *id).is_some())
            }
            _ => false,
        };
        if left {
            self.visit = self.visit.wrapping_add(1);
        }
        let effects = renamed
            .then(|| {
                Effect::NameHost(Saved {
                    ticket: next.ticket.clone(),
                    name: next.saved.clone(),
                })
            })
            .into_iter()
            .collect();
        self.hosts.insert(host, next);
        (self, effects)
    }

    fn replace(&mut self, shown: ShownPlace) {
        self.shown = shown;
        self.visit = self.visit.wrapping_add(1);
    }

    fn show(mut self, place: Place) -> Self {
        if self.hosts.is_empty() || place == self.shown.place() {
            return self;
        }
        self.recent = place.host().or(self.recent);
        let shown = match place {
            Place::Sessions => ShownPlace::Sessions,
            Place::AddHost => {
                ShownPlace::AddHost(Pairing::new(self.first_error.clone()))
            }
            Place::NewSession => ShownPlace::NewSession(Form::on(self.recent)),
            Place::Credentials => ShownPlace::Credentials(BTreeMap::new()),
            Place::Projects => ShownPlace::Projects(Titles::default()),
            Place::Session(host, id) => {
                let draft = self
                    .hosts
                    .get(&host)
                    .is_some_and(|known| {
                        known.fold.view.sessions.contains_key(&id)
                    })
                    .then(Pane::default);
                ShownPlace::Session(host, id, draft)
            }
            Place::Issue(host, id) => {
                let draft = self
                    .hosts
                    .get(&host)
                    .and_then(|known| issues::find(known, id))
                    .map(|_| IssueDraft::default());
                ShownPlace::Issue(host, id, draft)
            }
        };
        self.replace(shown);
        self
    }

    fn form(mut self, intent: crate::FormIntent) -> (Self, Vec<Effect>) {
        let ShownPlace::NewSession(form) = &mut self.shown else {
            return (self, Vec::new());
        };
        let (next, effects) =
            mem::take(form).update(intent, &self.hosts, self.visit);
        *form = next;
        (self, effects)
    }

    fn credential(
        mut self,
        host: HostId,
        intent: crate::CredentialIntent,
    ) -> (Self, Vec<Effect>) {
        let (ShownPlace::Credentials(forms), Some(known)) =
            (&mut self.shown, self.hosts.get(&host))
        else {
            return (self, Vec::new());
        };
        let kept = forms.entry(host).or_default();
        let (next, effects) = mem::take(kept).update(
            intent,
            At {
                host,
                visit: self.visit,
            },
            &known.fold.catalog,
        );
        *kept = next;
        (self, effects)
    }

    fn session(mut self, intent: SessionIntent) -> (Self, Vec<Effect>) {
        let ShownPlace::Session(host, id, Some(pane)) = &mut self.shown else {
            return (self, Vec::new());
        };
        let Some(known) = self.hosts.get(host) else {
            return (self, Vec::new());
        };
        let sessions = &known.fold.view.sessions;
        let Some(session) = sessions.get(id) else {
            return (self, Vec::new());
        };
        let at = At {
            host: *host,
            visit: self.visit,
        };
        let (next, effects) = mem::take(pane).update(intent, *id, session, at);
        *pane = next;
        (self, effects)
    }

    fn new_issue(
        mut self,
        intent: crate::NewIssueIntent,
    ) -> (Self, Vec<Effect>) {
        let ShownPlace::Projects(titles) = &mut self.shown else {
            return (self, Vec::new());
        };
        let (next, effects) = mem::take(titles).new_issue(intent, self.visit);
        *titles = next;
        (self, effects)
    }

    fn issue(mut self, intent: IssueIntent) -> (Self, Vec<Effect>) {
        let ShownPlace::Issue(host, id, Some(draft)) = &mut self.shown else {
            return (self, Vec::new());
        };
        let Some((_, issue)) = self
            .hosts
            .get(host)
            .and_then(|known| issues::find(known, *id))
        else {
            return (self, Vec::new());
        };
        let shown = Shown {
            at: At {
                host: *host,
                visit: self.visit,
            },
            issue,
        };
        let (next, effects) = mem::take(draft).update(intent, &shown);
        *draft = next;
        (self, effects)
    }

    fn replied(
        mut self,
        reply: Reply,
        answer: Result<CommandResult, String>,
    ) -> (Self, Vec<Effect>) {
        if reply.visit != self.visit {
            return (self, Vec::new());
        }
        let Some(known) = self.hosts.get(&reply.host) else {
            return (self, Vec::new());
        };
        let warning = match &answer {
            Ok(CommandResult::IssueWritten { warnings, .. })
                if !warnings.is_empty() =>
            {
                Some(warnings.join("; "))
            }
            _ => None,
        };
        match (&mut self.shown, reply.to) {
            (_, ReplyTo::Notice) => self.notice = refusal(answer),
            (ShownPlace::Issue(_, _, Some(draft)), ReplyTo::Issue(field)) => {
                draft.answered(field, refusal(answer));
            }
            (ShownPlace::Session(_, _, Some(_)), ReplyTo::Effort(shown)) => {
                return self.effort_refused(shown, answer);
            }
            (ShownPlace::NewSession(form), ReplyTo::Start(starts)) => {
                if let Some(session) =
                    form.answered(reply.host, starts, known, answer)
                {
                    self = self.show(Place::Session(reply.host, session));
                }
            }
            (ShownPlace::Session(_, _, Some(pane)), ReplyTo::Message) => {
                self.notice = refusal(answer);
                pane.message_answered(self.notice.is_none());
            }
            (ShownPlace::Projects(_), ReplyTo::Created) => {
                if let Ok(CommandResult::IssueWritten { issue, .. }) = answer {
                    self = self.show(Place::Issue(reply.host, issue));
                } else {
                    self.notice = refusal(answer);
                }
            }
            (
                ShownPlace::Credentials(forms),
                ReplyTo::Instance(subject, done),
            ) => {
                if let Some(host_forms) = forms.get_mut(&reply.host) {
                    *host_forms = mem::take(host_forms).replied(
                        subject,
                        done,
                        answer,
                        &known.name(),
                    );
                }
            }
            (
                ShownPlace::Session(_, _, Some(pane)),
                ReplyTo::Diff {
                    checkpoint,
                    request,
                },
            ) => {
                *pane = mem::take(pane).diffed(
                    checkpoint,
                    request,
                    answer,
                    &known.name(),
                );
            }
            _ => {}
        }
        if warning.is_some() {
            self.notice = warning;
        }
        (self, Vec::new())
    }

    /// A refused effort change goes to the notice, and the picker back to
    /// the effort the session still has.
    fn effort_refused(
        self,
        shown: Option<Effort>,
        answer: Result<CommandResult, String>,
    ) -> (Self, Vec<Effect>) {
        match refusal(answer) {
            Some(reason) => (
                Self {
                    notice: Some(reason),
                    ..self
                },
                Vec::from([Effect::ShowEffort(
                    shown.map_or_else(String::new, |kept| {
                        kept.wire().to_owned()
                    }),
                )]),
            ),
            None => (self, Vec::new()),
        }
    }

    pub(crate) fn screen(&self) -> Screen {
        if let ShownPlace::AddHost(pairing) = &self.shown
            && self.hosts.is_empty()
        {
            return Screen::Pair(pairing.screen());
        }
        let place = self.shown.place();
        let detail = self.detail();
        Screen::Hosts(Box::new(HostsScreen {
            links: self
                .hosts
                .iter()
                .map(|(id, host)| Dial {
                    host: *id,
                    key: host.ticket.to_text(),
                    ticket: host.ticket.clone(),
                })
                .collect(),
            shown: match detail {
                Detail::Placeholder(_) => "list",
                Detail::Pair(_)
                | Detail::NewSession(_)
                | Detail::Credentials(_)
                | Detail::Projects(_)
                | Detail::Session(_)
                | Detail::Issue(_) => "detail",
            },
            add_current: place == Place::AddHost,
            credentials_current: place == Place::Credentials,
            projects_current: matches!(
                place,
                Place::Projects | Place::Issue(..)
            ),
            list: sessions::list(&self.hosts, place),
            detail,
        }))
    }

    fn detail(&self) -> Detail {
        match &self.shown {
            ShownPlace::AddHost(pairing) => Detail::Pair(pairing.screen()),
            ShownPlace::NewSession(form) => {
                Detail::NewSession(Box::new(form.screen(&self.hosts)))
            }
            ShownPlace::Credentials(forms) => {
                let empty = Forms::default();
                Detail::Credentials(Box::new(CredentialsScreen {
                    hosts: by_name(&self.hosts)
                        .into_iter()
                        .map(|(id, host)| {
                            forms.get(&id).unwrap_or(&empty).screen(id, host)
                        })
                        .collect(),
                }))
            }
            ShownPlace::Projects(titles) => Detail::Projects(Box::new(
                projects::screen(&self.hosts, titles, self.notice.clone()),
            )),
            ShownPlace::Issue(host, id, Some(draft)) => self
                .hosts
                .get(host)
                .and_then(|known| {
                    issues::screen(
                        (*host, known),
                        *id,
                        (draft, &self.hosts),
                        self.notice.clone(),
                    )
                })
                .map_or(
                    Detail::Placeholder(
                        "That issue is gone from its host's home.",
                    ),
                    |screen| Detail::Issue(Box::new(screen)),
                ),
            ShownPlace::Issue(_, _, None) => {
                Detail::Placeholder("That issue is gone from its host's home.")
            }
            ShownPlace::Session(host, id, Some(pane)) => self
                .hosts
                .get(host)
                .and_then(|known| {
                    let session = known.fold.view.sessions.get(id)?;
                    Some(Detail::Session(Box::new(pane.screen(
                        *id,
                        session,
                        (known.fold.instances.as_ref(), &known.fold.catalog),
                        (self.notice.clone(), known.name()),
                    ))))
                })
                .unwrap_or_else(session_placeholder),
            ShownPlace::Sessions | ShownPlace::Session(_, _, None) => {
                session_placeholder()
            }
        }
    }
}

fn sync_draft<D: Default>(draft: &mut Option<D>, visible: bool) -> bool {
    if visible {
        draft.get_or_insert_with(D::default);
        false
    } else {
        draft.take().is_some()
    }
}

const fn session_placeholder() -> Detail {
    Detail::Placeholder("Choose a session, or start a new one.")
}

impl Pairing {
    const fn new(error: Option<String>) -> Self {
        Self {
            text: String::new(),
            error,
        }
    }

    fn screen(&self) -> Pair {
        Pair {
            text: self.text.clone(),
            connect_disabled: self.text.trim().is_empty(),
            error: self.error.clone(),
        }
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new(Opening::default())
    }
}

/// A command's answer, when it is a refusal or never came.
fn refusal(answer: Result<CommandResult, String>) -> Option<String> {
    match answer {
        Ok(
            CommandResult::Accepted
            | CommandResult::Started { .. }
            | CommandResult::Diff(_)
            | CommandResult::IssueWritten { .. },
        ) => None,
        Ok(CommandResult::Rejected(rejection)) => Some(rejection.label()),
        Ok(
            CommandResult::PushPublicKey(_) | CommandResult::PushUnavailable,
        ) => Some(String::from("the host returned a notification answer")),
        Ok(
            CommandResult::FleetStatus(_)
            | CommandResult::FleetInvite(_)
            | CommandResult::FleetPairing(_),
        ) => Some(String::from("The host gave an unexpected answer.")),
        Err(error) => Some(error),
    }
}

fn invalid(error: &TicketError) -> String {
    format!("That join link is not valid: {error}")
}
