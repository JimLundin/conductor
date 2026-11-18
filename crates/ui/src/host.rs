//! One host's screens: the status line, the session list and the detail
//! pane, and what each intent does to them.

use crate::prelude::*;
use client::host_view::{HostView, SessionView};
use client::link::{LinkState, Status, Update, shown_after};
use proto::{Catalog, CommandResult, Effort, Providers, SessionId, Ticket};

use crate::credentials::{CredentialsPane, Forms};
use crate::new_session::{Form, NewSessionForm};
use crate::session::{Pane, SessionPane};
use crate::sessions::{self, SessionList};
use crate::{Effect, Intent, Place, Reply, ReplyTo, SessionIntent};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostScreen {
    /// The host the link dials.
    pub ticket: Ticket,
    /// The pane a phone shows, `list` or `detail`. A wide window shows
    /// both.
    pub shown: &'static str,
    pub status: StatusLine,
    pub list: SessionList,
    pub detail: Detail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusLine {
    /// The link's state, also its `data-state`.
    pub state: String,
    pub host: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detail {
    Placeholder(&'static str),
    NewSession(Box<NewSessionForm>),
    Credentials(Box<CredentialsPane>),
    Session(Box<SessionPane>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    ticket: Ticket,
    status: Option<Status>,
    view: HostView,
    place: Place,
    /// The last command's refusal, shown under any session.
    notice: Option<String>,
    /// Counts the places shown. A form left since it sent a command
    /// ignores the answer.
    visit: u64,
    form: Form,
    credentials: Forms,
    pane: Pane,
}

impl Host {
    pub fn new(ticket: Ticket) -> Self {
        Self {
            ticket,
            status: None,
            view: HostView::empty(),
            place: Place::Sessions,
            notice: None,
            visit: 0,
            form: Form::default(),
            credentials: Forms::default(),
            pane: Pane::default(),
        }
    }

    pub fn update(self, intent: Intent) -> (Self, Vec<Effect>) {
        match intent {
            Intent::Link(update) => (self.linked(update), Vec::new()),
            Intent::Show(place) => (self.show(place), Vec::new()),
            Intent::DismissNotice => (
                Self {
                    notice: None,
                    ..self
                },
                Vec::new(),
            ),
            Intent::Replied(reply, answer) => self.replied(reply, answer),
            Intent::Form(change) => {
                let (form, effects) = self.form.update(
                    change,
                    providers(self.status.as_ref()),
                    self.visit,
                );
                (Self { form, ..self }, effects)
            }
            Intent::Credential(change) => {
                let (credentials, effects) = self.credentials.update(
                    change,
                    self.visit,
                    &catalog(self.status.as_ref()),
                );
                (
                    Self {
                        credentials,
                        ..self
                    },
                    effects,
                )
            }
            Intent::Session(change) => self.session(change),
            Intent::TypeJoin(_) | Intent::Join(_) | Intent::ChangeHost => {
                (self, Vec::new())
            }
        }
    }

    fn linked(self, update: Update) -> Self {
        let (status, view) = shown_after(self.status, self.view, update);
        Self {
            status,
            view,
            ..self
        }
    }

    /// Showing another place drops what the last one held: its drafts, the
    /// form's fields and the open diffs.
    fn show(self, place: Place) -> Self {
        if place == self.place {
            return self;
        }
        Self {
            place,
            visit: self.visit.wrapping_add(1),
            form: Form::default(),
            credentials: Forms::default(),
            pane: Pane::default(),
            ..self
        }
    }

    fn session(self, intent: SessionIntent) -> (Self, Vec<Effect>) {
        let Some((id, session)) = selected(&self.view, self.place) else {
            return (self, Vec::new());
        };
        let (pane, effects) = self.pane.update(intent, id, session, self.visit);
        (Self { pane, ..self }, effects)
    }

    fn replied(
        self,
        reply: Reply,
        answer: Result<CommandResult, String>,
    ) -> (Self, Vec<Effect>) {
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
            (ReplyTo::Start, _) => (self.started(answer, current), Vec::new()),
            (ReplyTo::Credential(provider, done), true) => {
                let credentials =
                    self.credentials.replied(provider, done, answer);
                (
                    Self {
                        credentials,
                        ..self
                    },
                    Vec::new(),
                )
            }
            (
                ReplyTo::Diff {
                    checkpoint,
                    request,
                },
                true,
            ) => {
                let pane = self.pane.diffed(checkpoint, request, answer);
                (Self { pane, ..self }, Vec::new())
            }
            (ReplyTo::Credential(..) | ReplyTo::Diff { .. }, false) => {
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

    /// A started session opens; the form shows any refusal.
    fn started(
        self,
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
            Some(session) => answered.show(Place::Session(session)),
            None => answered,
        }
    }

    pub fn screen(&self) -> HostScreen {
        let detail = self.detail();
        HostScreen {
            ticket: self.ticket.clone(),
            shown: match detail {
                Detail::Placeholder(_) => "list",
                Detail::NewSession(_)
                | Detail::Credentials(_)
                | Detail::Session(_) => "detail",
            },
            status: self.status_line(),
            list: sessions::list(
                &self.view,
                self.place,
                &catalog(self.status.as_ref()),
            ),
            detail,
        }
    }

    /// A session the host no longer reports (after a reset) falls back to
    /// the placeholder, and a phone to the list, its only way out.
    fn detail(&self) -> Detail {
        let known = providers(self.status.as_ref());
        let catalog = catalog(self.status.as_ref());
        match (self.place, selected(&self.view, self.place)) {
            (Place::NewSession, _) => Detail::NewSession(Box::new(
                self.form.screen(&self.view, known, &catalog),
            )),
            (Place::Credentials, _) => {
                let host = self
                    .status
                    .as_ref()
                    .and_then(|status| status.host.clone())
                    .unwrap_or_else(|| String::from("the host"));
                Detail::Credentials(Box::new(
                    self.credentials.screen(&host, known, &catalog),
                ))
            }
            (Place::Session(_), Some((id, session))) => {
                Detail::Session(Box::new(self.pane.screen(
                    id,
                    session,
                    known,
                    &catalog,
                    self.notice.clone(),
                )))
            }
            (Place::Sessions | Place::Session(_), _) => {
                Detail::Placeholder("Choose a session, or start a new one.")
            }
        }
    }

    fn status_line(&self) -> StatusLine {
        let shown = self.status.clone().unwrap_or_else(|| Status {
            state: LinkState::Disconnected,
            host: None,
            providers: None,
            catalog: Catalog::default(),
            error: None,
        });
        StatusLine {
            state: format!("{:?}", shown.state),
            host: shown.host.unwrap_or_else(|| String::from("host")),
            error: shown.error,
        }
    }
}

fn providers(status: Option<&Status>) -> Option<&Providers> {
    status.and_then(|known| known.providers.as_ref())
}

/// How each provider presents itself, once the host has said.
fn catalog(status: Option<&Status>) -> Catalog {
    status.map_or_default(|known| known.catalog.clone())
}

/// The session shown, if the host still reports it.
fn selected(
    view: &HostView,
    place: Place,
) -> Option<(SessionId, &SessionView)> {
    match place {
        Place::Session(id) => {
            view.sessions.get(&id).map(|session| (id, session))
        }
        Place::Sessions | Place::NewSession | Place::Credentials => None,
    }
}

/// A command's answer, when it is a refusal or never came.
fn refusal(answer: Result<CommandResult, String>) -> Option<String> {
    match answer {
        Ok(
            CommandResult::Accepted
            | CommandResult::Started { .. }
            | CommandResult::Providers(_)
            | CommandResult::Diff(_),
        ) => None,
        Ok(CommandResult::Rejected(rejection)) => Some(rejection.label()),
        Err(error) => Some(error),
    }
}
