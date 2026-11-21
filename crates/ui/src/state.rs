//! Every paired host, and what the person has open over them: the place
//! shown, its drafts and forms, and what each intent does to them. What a
//! place holds belongs to the host and session it shows, and goes when
//! another place is shown.

use alloc::collections::BTreeMap;

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
use crate::{At, Effect, Intent, Opening, Pair, Place, Reply, ReplyTo, Screen};

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
        let mut opened = Self {
            hosts,
            shown,
            recent: None,
            notice: None,
            visit: 0,
            first_error,
        };
        if let Some(Ok(ticket)) = opening.joined {
            opened.pair(ticket);
        }
        opened
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

    #[must_use]
    pub fn update(&mut self, intent: Intent) -> Vec<Effect> {
        let at = |host| At {
            host,
            visit: self.visit,
        };
        match (intent, &mut self.shown) {
            (Intent::TypeJoin(text), ShownPlace::AddHost(pairing)) => {
                pairing.text = text;
            }
            (Intent::Join(Ok(ticket)), ShownPlace::AddHost(_)) => {
                self.pair(ticket);
                return Vec::from([Effect::SaveHosts(self.saved())]);
            }
            (Intent::Join(Err(error)), ShownPlace::AddHost(pairing)) => {
                pairing.error = Some(invalid(&error));
            }
            (Intent::Forget(host), _) => return self.forget(host),
            (Intent::Link(host, update), _) => {
                return self.linked(host, update);
            }
            (Intent::Show(place), _) => self.show(place),
            (Intent::DismissNotice, _) => self.notice = None,
            (Intent::Form(change), ShownPlace::NewSession(form)) => {
                return form.update(change, &self.hosts, self.visit);
            }
            (
                Intent::Credential(host, change),
                ShownPlace::Credentials(forms),
            ) => {
                if let Some(known) = self.hosts.get(&host) {
                    return forms.entry(host).or_default().update(
                        change,
                        at(host),
                        &known.fold.catalog,
                    );
                }
            }
            (Intent::Project(host, change), ShownPlace::Projects(_))
                if self.hosts.contains_key(&host) =>
            {
                return change.send(at(host));
            }
            (
                Intent::Session(change),
                ShownPlace::Session(host, id, Some(pane)),
            ) => {
                if let Some(session) = self
                    .hosts
                    .get(host)
                    .and_then(|known| known.fold.view.sessions.get(id))
                {
                    return pane.update(change, *id, session, at(*host));
                }
            }
            (Intent::NewIssue(change), ShownPlace::Projects(titles)) => {
                return titles.new_issue(change, self.visit);
            }
            (
                Intent::Issue(change),
                ShownPlace::Issue(host, id, Some(draft)),
            ) => {
                if let Some((_, issue)) = self
                    .hosts
                    .get(host)
                    .and_then(|known| issues::find(known, *id))
                {
                    let shown = Shown {
                        at: at(*host),
                        issue,
                    };
                    return draft.update(change, &shown);
                }
            }
            (Intent::StartOn(host, issue), _) if !self.hosts.is_empty() => {
                self.replace(ShownPlace::NewSession(Form::on_issue(
                    host, issue,
                )));
            }
            (Intent::Replied(reply, answer), _) => {
                return self.replied(reply, answer);
            }
            (
                Intent::TypeJoin(_)
                | Intent::Join(_)
                | Intent::Form(_)
                | Intent::Credential(..)
                | Intent::Project(..)
                | Intent::Session(_)
                | Intent::NewIssue(_)
                | Intent::Issue(_)
                | Intent::StartOn(..),
                _,
            ) => {}
        }
        Vec::new()
    }

    fn pair(&mut self, ticket: Ticket) {
        match self.hosts.entry(HostId::of(&ticket)) {
            alloc::collections::btree_map::Entry::Occupied(mut entry) => {
                entry.get_mut().ticket = ticket;
            }
            alloc::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(Host::new(ticket, None));
            }
        }
        if matches!(self.shown, ShownPlace::AddHost(_)) {
            self.replace(ShownPlace::Sessions);
        }
    }

    fn forget(&mut self, host: HostId) -> Vec<Effect> {
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
        Vec::from([Effect::SaveHosts(self.saved())])
    }

    fn linked(&mut self, host: HostId, update: Update) -> Vec<Effect> {
        let Some(known) = self.hosts.get_mut(&host) else {
            return Vec::new();
        };
        let renamed = known.linked(update);
        let left = match &mut self.shown {
            ShownPlace::Session(selected, id, draft) if *selected == host => {
                sync_draft(draft, known.fold.view.sessions.contains_key(id))
            }
            ShownPlace::Issue(selected, id, draft) if *selected == host => {
                sync_draft(draft, issues::find(known, *id).is_some())
            }
            _ => false,
        };
        if left {
            self.visit = self.visit.wrapping_add(1);
        }
        renamed
            .then(|| Effect::SaveHosts(self.saved()))
            .into_iter()
            .collect()
    }

    fn replace(&mut self, shown: ShownPlace) {
        self.shown = shown;
        self.visit = self.visit.wrapping_add(1);
    }

    fn show(&mut self, place: Place) {
        if self.hosts.is_empty() || place == self.shown.place() {
            return;
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
    }

    fn replied(
        &mut self,
        reply: Reply,
        answer: Result<CommandResult, String>,
    ) -> Vec<Effect> {
        if reply.visit != self.visit {
            return Vec::new();
        }
        let Some(known) = self.hosts.get(&reply.host) else {
            return Vec::new();
        };
        match (&mut self.shown, reply.to) {
            (_, ReplyTo::Notice) => self.notice = refusal(answer),
            (ShownPlace::Session(_, _, Some(_)), ReplyTo::Effort(shown)) => {
                return self.effort_refused(shown, answer);
            }
            (ShownPlace::NewSession(form), ReplyTo::Start) => {
                if let Ok(CommandResult::Started { session }) = answer {
                    self.show(Place::Session(reply.host, session));
                } else {
                    form.answered(known, answer);
                }
            }
            (ShownPlace::Projects(_), ReplyTo::Created) => {
                if let Ok(CommandResult::IssueCreated { issue }) = answer {
                    self.show(Place::Issue(reply.host, issue));
                } else {
                    self.notice = refusal(answer);
                }
            }
            (
                ShownPlace::Credentials(forms),
                ReplyTo::Instance(subject, done),
            ) => {
                if let Some(host_forms) = forms.get_mut(&reply.host) {
                    host_forms.replied(subject, done, answer, &known.name());
                }
            }
            (
                ShownPlace::Session(_, _, Some(pane)),
                ReplyTo::Diff {
                    checkpoint,
                    request,
                },
            ) => {
                pane.diffed(checkpoint, request, answer, &known.name());
            }
            _ => {}
        }
        Vec::new()
    }

    /// A refused effort change goes to the notice, and the picker back to
    /// the effort the session still has.
    fn effort_refused(
        &mut self,
        shown: Option<Effort>,
        answer: Result<CommandResult, String>,
    ) -> Vec<Effect> {
        let Some(reason) = refusal(answer) else {
            return Vec::new();
        };
        self.notice = Some(reason);
        Vec::from([Effect::ShowEffort(
            shown.map_or_else(String::new, |kept| kept.wire().to_owned()),
        )])
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
            | CommandResult::Instances(_)
            | CommandResult::Diff(_)
            | CommandResult::IssueCreated { .. },
        ) => None,
        Ok(CommandResult::Rejected(rejection)) => Some(rejection.label()),
        Err(error) => Some(error),
    }
}

fn invalid(error: &TicketError) -> String {
    format!("That join link is not valid: {error}")
}
