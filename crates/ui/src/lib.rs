//! The app's screen as data (architecture §8). [`State`] is what the app
//! holds, over every paired host; [`update`] takes what a person or a
//! host's link does, as an [`Intent`], and returns the next state and the
//! [`Effect`]s the shell performs; [`view`] describes the whole
//! [`Screen`], so the app's markup only draws it. It is pure and `no_std`.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod checkpoints;
mod credentials;
mod host;
mod new_session;
mod prelude;
mod projects;
mod saved;
mod session;
mod sessions;
mod state;
mod transcript;

#[cfg(test)]
mod tests;

use crate::prelude::*;
use client::link::Update;
use proto::{
    CheckpointId, CommandBody, CommandResult, Effort, SessionId, Ticket,
    TicketError,
};

pub use checkpoints::{Diff, DiffFile, DiffPanel, Patch, TurnChanges};
pub use credentials::{
    AddForm, ClearButton, CredentialIntent, CredentialsPane, CredentialsScreen,
    DeviceCode, FieldView, InstanceCredentials, KindForm, PasteForm,
    RenameForm, SignInButton, SignInView,
};
pub use host::{HostId, StatusLine};
pub use new_session::{
    Choice, FormIntent, Hint, InstanceChoice, NewSessionForm, Suggestion,
};
pub use projects::{
    CheckoutRow, CloneButton, HomeLine, ProjectGroup, ProjectIntent,
    ProjectsScreen, when,
};
pub use saved::{Saved, Stored, decode, encode};
pub use session::{
    Composer, EffortControl, Entry, Footer, Mount, SessionIntent, SessionPane,
    TaskRow, Tasks, UndoNote, WorkspaceContext,
};
pub use sessions::{HostGroup, Listing, SessionList, SessionRow};
pub use state::{Detail, Dial, HostsScreen, State};
pub use transcript::{
    Action, AgentGroup, AnsweredQuestion, ApprovalCard, Ask, OptionButton,
    QuestionBody, QuestionCard, QuestionField, QuestionForm, Row, ToolDetail,
    ToolFile, ToolLine,
};

use credentials::{Done, Subject};

/// What the app opens on: what local storage holds, and the join link in
/// the address bar, if any.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Opening {
    pub stored: Stored,
    pub joined: Option<Result<Ticket, TicketError>>,
}

/// What fills the detail pane, and on a phone the whole window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Sessions,
    /// The pairing form, for one more host.
    AddHost,
    NewSession,
    Credentials,
    Projects,
    Session(HostId, SessionId),
}

impl Place {
    /// The host a session's place is on.
    const fn host(self) -> Option<HostId> {
        match self {
            Self::Session(host, _) => Some(host),
            Self::Sessions
            | Self::AddHost
            | Self::NewSession
            | Self::Credentials
            | Self::Projects => None,
        }
    }
}

/// Everything a person can do, and what the links and the hosts' answers
/// bring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    /// The pairing form's text.
    TypeJoin(String),
    /// The pairing form sent, with what its text parsed to. A host paired
    /// again keeps its place and takes the new ticket.
    Join(Result<Ticket, TicketError>),
    /// Forget this host; the last one gone, pair again.
    Forget(HostId),
    Link(HostId, Update),
    Show(Place),
    DismissNotice,
    Form(FormIntent),
    Credential(HostId, CredentialIntent),
    /// A home command for this host.
    Project(HostId, ProjectIntent),
    /// Acts on the session shown.
    Session(SessionIntent),
    /// A host's answer to a command an [`Effect::Send`] sent, or why none
    /// came.
    Replied(Reply, Result<CommandResult, String>),
}

/// What the shell does for an [`update`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Send `body` to `host` and return its answer as
    /// [`Intent::Replied`] with `reply`.
    Send {
        host: HostId,
        body: CommandBody,
        reply: Reply,
    },
    /// Keep these hosts in local storage, as [`encode`] writes them.
    SaveHosts(Vec<Saved>),
    Focus(Focus),
    /// Put the session's effort picker back on this value, which a refused
    /// change left behind.
    ShowEffort(String),
    /// Put this text on the clipboard.
    Copy(String),
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
    host: HostId,
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
    Instance(Subject, Done),
    Diff {
        checkpoint: CheckpointId,
        request: u64,
    },
}

/// The host a command goes to, and the visit of the place that sent it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct At {
    host: HostId,
    visit: u64,
}

impl At {
    const fn send(self, body: CommandBody, to: ReplyTo) -> Effect {
        Effect::Send {
            host: self.host,
            body,
            reply: Reply {
                host: self.host,
                visit: self.visit,
                to,
            },
        }
    }
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
    /// No host is paired.
    Pair(Pair),
    Hosts(Box<HostsScreen>),
}

/// The pairing form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pair {
    pub text: String,
    pub connect_disabled: bool,
    pub error: Option<String>,
}

#[must_use]
pub fn update(state: State, intent: Intent) -> (State, Vec<Effect>) {
    state.update(intent)
}

#[must_use]
pub fn view(state: &State) -> Screen {
    state.screen()
}
