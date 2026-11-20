//! Every paired host, and what the person has open over them: the place
//! shown, its drafts and forms, and what each intent does to them. What a
//! place holds belongs to the host and session it shows, and goes when
//! another place is shown.

use alloc::collections::BTreeMap;

use crate::prelude::*;
use client::host_view::SessionView;
use client::link::Update;
use proto::{CommandResult, Effort, SessionId, Ticket, TicketError};

use crate::credentials::{CredentialsScreen, Forms};
use crate::host::{Host, HostId, by_name};
use crate::issues::{self, Drafts, IssueScreen, Shown};
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
    place: Place,
    /// The host of the last session shown, where a new session starts.
    recent: Option<HostId>,
    /// The last command's refusal, shown under any session.
    notice: Option<String>,
    /// Counts the places shown. A form left since it sent a command
    /// ignores the answer.
    visit: u64,
    form: Form,
    credentials: BTreeMap<HostId, Forms>,
    pane: Pane,
    issues: Drafts,
    pairing: Pairing,
    /// Why the join link the app opened on was refused. The pairing form
    /// says so each time it opens.
    first_error: Option<String>,
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
        let hosts = decode(&opening.stored)
            .into_iter()
            .map(|saved| {
                (
                    HostId::of(&saved.ticket),
                    Host::new(saved.ticket, saved.name),
                )
            })
            .collect();
        let opened = Self {
            hosts,
            place: Place::Sessions,
            recent: None,
            notice: None,
            visit: 0,
            form: Form::default(),
            credentials: BTreeMap::new(),
            pane: Pane::default(),
            issues: Drafts::default(),
            pairing: Pairing {
                text: String::new(),
                error: first_error.clone(),
            },
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

    pub(crate) fn update(self, intent: Intent) -> (Self, Vec<Effect>) {
        match intent {
            Intent::TypeJoin(text) => (
                Self {
                    pairing: Pairing {
                        text,
                        error: self.pairing.error.clone(),
                    },
                    ..self
                },
                Vec::new(),
            ),
            Intent::Join(Ok(ticket)) => {
                let next = self.paired(ticket);
                let saved = next.saved();
                (next, Vec::from([Effect::SaveHosts(saved)]))
            }
            Intent::Join(Err(error)) => (
                Self {
                    pairing: Pairing {
                        text: self.pairing.text.clone(),
                        error: Some(invalid(&error)),
                    },
                    ..self
                },
                Vec::new(),
            ),
            Intent::Forget(host) => self.forget(host),
            Intent::Link(host, update) => self.linked(host, update),
            Intent::Show(place) => (self.show(place), Vec::new()),
            Intent::DismissNotice => (
                Self {
                    notice: None,
                    ..self
                },
                Vec::new(),
            ),
            Intent::Form(change) => {
                let (form, effects) =
                    self.form.update(change, &self.hosts, self.visit);
                (Self { form, ..self }, effects)
            }
            Intent::Credential(host, change) => self.credential(host, change),
            Intent::Project(host, change) => {
                let at = At {
                    host,
                    visit: self.visit,
                };
                (self, change.send(at))
            }
            Intent::Session(change) => self.session(change),
            Intent::NewIssue(change) => {
                let (issues, effects) =
                    self.issues.clone().new_issue(change, self.visit);
                (Self { issues, ..self }, effects)
            }
            Intent::Issue(change) => self.issue(change),
            Intent::StartOn(host, issue) => {
                let shown = self.show(Place::NewSession);
                (
                    Self {
                        form: Form::on_issue(host, issue),
                        ..shown
                    },
                    Vec::new(),
                )
            }
            Intent::Replied(reply, answer) => self.replied(reply, answer),
        }
    }

    /// Adds the host, or gives a known one its new ticket, and leaves the
    /// pairing form.
    fn paired(mut self, ticket: Ticket) -> Self {
        let id = HostId::of(&ticket);
        let host = match self.hosts.remove(&id) {
            Some(known) => Host { ticket, ..known },
            None => Host::new(ticket, None),
        };
        self.hosts.insert(id, host);
        let shown = match self.place {
            Place::AddHost => self.show(Place::Sessions),
            Place::Sessions
            | Place::NewSession
            | Place::Credentials
            | Place::Projects
            | Place::Session(..)
            | Place::Issue(..) => self,
        };
        Self {
            pairing: Pairing::default(),
            ..shown
        }
    }

    /// Drops the host and what was open on it. With none left, the app
    /// asks to pair again.
    fn forget(mut self, host: HostId) -> (Self, Vec<Effect>) {
        self.hosts.remove(&host);
        self.credentials.remove(&host);
        self.recent = self.recent.filter(|recent| *recent != host);
        let left =
            match (self.place.host() == Some(host), self.hosts.is_empty()) {
                (true, _) | (_, true) => self.show(Place::Sessions),
                (false, false) => self,
            };
        let saved = left.saved();
        let error = left.first_error.clone();
        (
            Self {
                pairing: Pairing {
                    text: String::new(),
                    error,
                },
                ..left
            },
            Vec::from([Effect::SaveHosts(saved)]),
        )
    }

    fn linked(mut self, host: HostId, update: Update) -> (Self, Vec<Effect>) {
        let Some(known) = self.hosts.remove(&host) else {
            return (self, Vec::new());
        };
        let (next, renamed) = known.linked(update);
        self.hosts.insert(host, next);
        let effects = renamed
            .then(|| Effect::SaveHosts(self.saved()))
            .into_iter()
            .collect();
        (self, effects)
    }

    /// Showing another place drops what the last one held: its drafts, the
    /// forms' fields and the open diffs.
    fn show(self, place: Place) -> Self {
        if place == self.place {
            return self;
        }
        let recent = place.host().or(self.recent);
        let error = self.first_error.clone();
        Self {
            place,
            recent,
            visit: self.visit.wrapping_add(1),
            form: Form::on(recent),
            credentials: BTreeMap::new(),
            pane: Pane::default(),
            issues: Drafts::default(),
            pairing: Pairing {
                text: String::new(),
                error,
            },
            ..self
        }
    }

    fn credential(
        mut self,
        host: HostId,
        intent: crate::CredentialIntent,
    ) -> (Self, Vec<Effect>) {
        let Some(known) = self.hosts.get(&host) else {
            return (self, Vec::new());
        };
        let kept = self.credentials.remove(&host).unwrap_or_default();
        let (forms, effects) = kept.update(
            intent,
            At {
                host,
                visit: self.visit,
            },
            &known.fold.catalog,
        );
        self.credentials.insert(host, forms);
        (self, effects)
    }

    fn session(self, intent: SessionIntent) -> (Self, Vec<Effect>) {
        let Place::Session(host, id) = self.place else {
            return (self, Vec::new());
        };
        let Some(session) = self
            .hosts
            .get(&host)
            .and_then(|known| known.fold.view.sessions.get(&id))
        else {
            return (self, Vec::new());
        };
        let at = At {
            host,
            visit: self.visit,
        };
        let (pane, effects) = self.pane.update(intent, id, session, at);
        (Self { pane, ..self }, effects)
    }

    fn issue(self, intent: IssueIntent) -> (Self, Vec<Effect>) {
        let Place::Issue(host, id) = self.place else {
            return (self, Vec::new());
        };
        let Some((_, issue)) = self
            .hosts
            .get(&host)
            .and_then(|known| issues::find(known, id))
        else {
            return (self, Vec::new());
        };
        let shown = Shown {
            at: At {
                host,
                visit: self.visit,
            },
            issue,
        };
        let (drafts, effects) = self.issues.clone().update(intent, &shown);
        (
            Self {
                issues: drafts,
                ..self
            },
            effects,
        )
    }

    /// The session shown, if its host still reports it.
    fn selected(&self) -> Option<(HostId, SessionId, &SessionView)> {
        let Place::Session(host, id) = self.place else {
            return None;
        };
        self.hosts
            .get(&host)
            .and_then(|known| known.fold.view.sessions.get(&id))
            .map(|session| (host, id, session))
    }

    fn replied(
        mut self,
        reply: Reply,
        answer: Result<CommandResult, String>,
    ) -> (Self, Vec<Effect>) {
        let Some(name) = self.hosts.get(&reply.host).map(Host::name) else {
            return (self, Vec::new());
        };
        let current = reply.visit == self.visit;
        match (reply.to, current) {
            (ReplyTo::Notice, _) => (
                Self {
                    notice: refusal(answer),
                    ..self
                },
                Vec::new(),
            ),
            (ReplyTo::Effort(shown), _) => self.effort_refused(shown, answer),
            (ReplyTo::Start, _) => {
                (self.started(reply.host, answer, current), Vec::new())
            }
            (ReplyTo::Created(project), _) => {
                (self.opened(reply.host, &project, answer), Vec::new())
            }
            (ReplyTo::Instance(subject, done), true) => {
                let forms = self
                    .credentials
                    .remove(&reply.host)
                    .unwrap_or_default()
                    .replied(subject, done, answer, &name);
                self.credentials.insert(reply.host, forms);
                (self, Vec::new())
            }
            (
                ReplyTo::Diff {
                    checkpoint,
                    request,
                },
                true,
            ) => {
                let pane = self.pane.diffed(checkpoint, request, answer, &name);
                (Self { pane, ..self }, Vec::new())
            }
            (ReplyTo::Instance(..) | ReplyTo::Diff { .. }, false) => {
                (self, Vec::new())
            }
        }
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

    /// A new issue opens, its title draft gone; a refusal is the notice.
    fn opened(
        self,
        host: HostId,
        project: &proto::ProjectId,
        answer: Result<CommandResult, String>,
    ) -> Self {
        match answer {
            Ok(CommandResult::IssueCreated { issue }) => {
                let issues = self.issues.clone().created(project);
                Self { issues, ..self }.show(Place::Issue(host, issue))
            }
            other => Self {
                notice: refusal(other),
                ..self
            },
        }
    }

    /// A started session opens; the form shows any refusal.
    fn started(
        self,
        host: HostId,
        answer: Result<CommandResult, String>,
        current: bool,
    ) -> Self {
        let opened = match &answer {
            Ok(CommandResult::Started { session }) => Some(*session),
            Ok(_) | Err(_) => None,
        };
        let answered = if current {
            Self {
                form: self.form.answered(answer),
                ..self
            }
        } else {
            self
        };
        match opened {
            Some(session) => answered.show(Place::Session(host, session)),
            None => answered,
        }
    }

    pub(crate) fn screen(&self) -> Screen {
        if self.hosts.is_empty() {
            return Screen::Pair(self.pair());
        }
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
            add_current: self.place == Place::AddHost,
            credentials_current: self.place == Place::Credentials,
            projects_current: matches!(
                self.place,
                Place::Projects | Place::Issue(..)
            ),
            list: sessions::list(&self.hosts, self.place),
            detail,
        }))
    }

    fn pair(&self) -> Pair {
        Pair {
            text: self.pairing.text.clone(),
            connect_disabled: self.pairing.text.trim().is_empty(),
            error: self.pairing.error.clone(),
        }
    }

    /// A session its host no longer reports (after a reset, or once the
    /// host is forgotten) falls back to the placeholder, and a phone to the
    /// list, its only way out.
    fn detail(&self) -> Detail {
        match (self.place, self.selected()) {
            (Place::AddHost, _) => Detail::Pair(self.pair()),
            (Place::NewSession, _) => {
                Detail::NewSession(Box::new(self.form.screen(&self.hosts)))
            }
            (Place::Credentials, _) => {
                let empty = Forms::default();
                Detail::Credentials(Box::new(CredentialsScreen {
                    hosts: by_name(&self.hosts)
                        .into_iter()
                        .map(|(id, host)| {
                            self.credentials
                                .get(&id)
                                .unwrap_or(&empty)
                                .screen(id, host)
                        })
                        .collect(),
                }))
            }
            (Place::Projects, _) => {
                Detail::Projects(Box::new(projects::screen(
                    &self.hosts,
                    &self.issues,
                    self.notice.clone(),
                )))
            }
            (Place::Issue(host, id), _) => self
                .hosts
                .get(&host)
                .and_then(|known| {
                    issues::screen(
                        (host, known),
                        id,
                        (&self.issues, &self.hosts),
                        self.notice.clone(),
                    )
                })
                .map_or(
                    Detail::Placeholder(
                        "That issue is gone from its host's home.",
                    ),
                    |screen| Detail::Issue(Box::new(screen)),
                ),
            (Place::Session(..), Some((host, id, session))) => {
                let known = self.hosts.get(&host);
                let empty_catalog = proto::Catalog::default();
                let catalog =
                    known.map_or(&empty_catalog, |host| &host.fold.catalog);
                Detail::Session(Box::new(self.pane.screen(
                    id,
                    session,
                    (
                        known.and_then(|host| host.fold.instances.as_ref()),
                        catalog,
                    ),
                    (self.notice.clone(), known.map_or_default(Host::name)),
                )))
            }
            (Place::Sessions | Place::Session(..), _) => {
                Detail::Placeholder("Choose a session, or start a new one.")
            }
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
