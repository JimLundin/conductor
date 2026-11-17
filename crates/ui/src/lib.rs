//! The app's screen as data (architecture §8). [`State`] is what the app
//! holds; [`update`] takes what a person or the link does, as an
//! [`Intent`], and returns the next state and the [`Effect`]s the shell
//! performs; [`view`] describes the whole [`Screen`], so the app's markup
//! only draws it. It is pure and `no_std`.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod checkpoints;
mod credentials;
mod host;
mod new_session;
mod prelude;
mod session;
mod sessions;
mod transcript;

#[cfg(test)]
mod tests;

use crate::prelude::*;
use client::link::Update;
use proto::{
    CheckpointId, CommandBody, CommandResult, Effort, Provider, SessionId,
    Ticket, TicketError,
};

pub use checkpoints::{Diff, DiffFile, DiffPanel, Patch, TurnChanges};
pub use credentials::{
    ClearButton, CredentialForm, CredentialIntent, CredentialsPane, Field,
    FieldView,
};
pub use host::{Detail, HostScreen, StatusLine};
pub use new_session::{
    Choice, FormIntent, Hint, NewSessionForm, ProviderChoice, Suggestion,
};
pub use session::{
    Composer, EffortControl, Entry, Footer, Mount, SessionIntent, SessionPane,
    TaskRow, Tasks, UndoNote, WorkspaceContext,
};
pub use sessions::{Listing, SessionList, SessionRow};
pub use transcript::{
    Action, AnsweredQuestion, ApprovalCard, Ask, OptionButton, QuestionBody,
    QuestionCard, QuestionField, QuestionForm, Row, ToolDetail, ToolFile,
    ToolLine,
};

use credentials::Done;
use host::Host;

/// Where the app opens: a host it knows, or the pairing form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opening {
    Host(Ticket),
    Unpaired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pairing: Pairing,
    /// Why the join link the app opened on was refused. The pairing form
    /// says so each time it opens.
    first_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Pairing {
    Unpaired { text: String, error: Option<String> },
    Paired(Box<Host>),
}

impl State {
    #[must_use]
    pub fn new(opening: Result<Opening, TicketError>) -> Self {
        let first_error = opening.as_ref().err().map(invalid);
        let pairing = match opening {
            Ok(Opening::Host(ticket)) => {
                Pairing::Paired(Box::new(Host::new(ticket)))
            }
            Ok(Opening::Unpaired) | Err(_) => Pairing::Unpaired {
                text: String::new(),
                error: first_error.clone(),
            },
        };
        Self {
            pairing,
            first_error,
        }
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new(Ok(Opening::Unpaired))
    }
}

/// What fills the detail pane, and on a phone the whole window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Sessions,
    NewSession,
    Credentials,
    Session(SessionId),
}

/// Everything a person can do, and what the link and the host's answers
/// bring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    /// The pairing form's text.
    TypeJoin(String),
    /// The pairing form sent, with what its text parsed to.
    Join(Result<Ticket, TicketError>),
    /// Forget this host and pair again.
    ChangeHost,
    Link(Update),
    Show(Place),
    DismissNotice,
    Form(FormIntent),
    Credential(CredentialIntent),
    Session(SessionIntent),
    /// The host's answer to a command an [`Effect::Send`] sent, or why none
    /// came.
    Replied(Reply, Result<CommandResult, String>),
}

/// What the shell does for an [`update`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Send `body` to the host and return its answer as
    /// [`Intent::Replied`] with `reply`.
    Send {
        body: CommandBody,
        reply: Reply,
    },
    SaveTicket(Ticket),
    ForgetTicket,
    Focus(Focus),
    /// Put the session's effort picker back on this value, which a refused
    /// change left behind.
    ShowEffort(String),
}

/// Where focus goes once a control it was on goes away.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Focus {
    /// The element with this id, now.
    Element(String),
    /// The element with this id, once the next frame is drawn.
    NextFrame(String),
    /// The composer if it takes input, else the undo note.
    AfterUndo,
    Composer,
}

/// Which command an answer belongs to. A form the person has left since
/// it sent the command ignores the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    visit: u64,
    to: ReplyTo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReplyTo {
    /// Only a refusal is worth showing, as the notice.
    Notice,
    /// A refused effort change puts the picker back on this effort.
    Effort(Option<Effort>),
    Start,
    Credential(Provider, Done),
    Diff {
        checkpoint: CheckpointId,
        request: u64,
    },
}

/// A key pressed in the composer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Keystroke {
    pub enter: bool,
    pub shift: bool,
    /// An input method is composing a character.
    pub composing: bool,
}

/// Return sends the message and Shift-Return starts a new line, except
/// while an input method is composing.
#[must_use]
pub const fn sends(key: Keystroke) -> bool {
    key.enter && !key.shift && !key.composing
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    Pair(Pair),
    Host(Box<HostScreen>),
}

/// Shown until a host is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pair {
    pub text: String,
    pub connect_disabled: bool,
    pub error: Option<String>,
}

#[must_use]
pub fn update(state: State, intent: Intent) -> (State, Vec<Effect>) {
    let State {
        pairing,
        first_error,
    } = state;
    let (next, effects) = match (pairing, intent) {
        (Pairing::Unpaired { error, .. }, Intent::TypeJoin(text)) => {
            (Pairing::Unpaired { text, error }, Vec::new())
        }
        (Pairing::Unpaired { .. }, Intent::Join(Ok(ticket))) => (
            Pairing::Paired(Box::new(Host::new(ticket.clone()))),
            Vec::from([Effect::SaveTicket(ticket)]),
        ),
        (Pairing::Unpaired { text, .. }, Intent::Join(Err(error))) => (
            Pairing::Unpaired {
                text,
                error: Some(invalid(&error)),
            },
            Vec::new(),
        ),
        (Pairing::Paired(_), Intent::ChangeHost) => (
            Pairing::Unpaired {
                text: String::new(),
                error: first_error.clone(),
            },
            Vec::from([Effect::ForgetTicket]),
        ),
        (Pairing::Paired(host), other) => {
            let (next, effects) = Host::update(*host, other);
            (Pairing::Paired(Box::new(next)), effects)
        }
        (unpaired @ Pairing::Unpaired { .. }, _) => (unpaired, Vec::new()),
    };
    (
        State {
            pairing: next,
            first_error,
        },
        effects,
    )
}

#[must_use]
pub fn view(state: &State) -> Screen {
    match &state.pairing {
        Pairing::Unpaired { text, error } => Screen::Pair(Pair {
            text: text.clone(),
            connect_disabled: text.trim().is_empty(),
            error: error.clone(),
        }),
        Pairing::Paired(host) => Screen::Host(Box::new(host.screen())),
    }
}

fn invalid(error: &TicketError) -> String {
    format!("That join link is not valid: {error}")
}
