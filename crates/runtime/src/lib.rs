pub mod agent;
mod plugin;

pub use plugin::{
    Configuring, CredentialSigningIn, Instance, Launch, Plugin,
    ProviderInstance, Settings,
};

use agents::Note;
use agents::runtime::{Command, Event};
use tokio::sync::mpsc;

#[derive(Debug)]
pub struct Runtime {
    pub commands: mpsc::Sender<Command>,
    pub events: mpsc::Receiver<Event>,
}

#[derive(Debug)]
struct Plug {
    commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<Event>,
}

#[must_use]
fn pair() -> (Runtime, Plug) {
    let (commands, received) = mpsc::channel(16);
    let (reported, events) = mpsc::channel(1024);
    (
        Runtime { commands, events },
        Plug {
            commands: received,
            events: reported,
        },
    )
}

pub fn log(note: &Note) {
    match note {
        Note::Unreadable { provider, error } => {
            tracing::error!(target: "agent", %provider, %error, "reading stdout failed");
        }
        Note::NotJson { provider, error } => {
            tracing::warn!(target: "agent", %provider, %error, "printed a line that is not JSON");
        }
        Note::Unmapped { provider, frame } => {
            tracing::trace!(target: "agent", %provider, %frame, "unmapped frame");
        }
        Note::UnmappedOnThread {
            provider,
            thread,
            frame,
        } => {
            tracing::trace!(target: "agent", %provider, %thread, %frame, "unmapped thread frame");
        }
        Note::ThreadSettings {
            provider,
            thread,
            settings,
        } => {
            tracing::debug!(target: "agent", %provider, %thread, %settings, "thread settings");
        }
        Note::UnknownResponse { provider, request } => {
            tracing::warn!(target: "agent", %provider, %request, "response for unknown request");
        }
        Note::Unsupported { provider, request } => {
            tracing::warn!(target: "agent", %provider, %request, "refused a request conductor does not handle");
        }
        Note::Unwritten {
            provider,
            writing,
            error,
        } => {
            tracing::error!(target: "agent", %provider, %writing, %error, "write failed");
        }
        Note::SecretDeclined { provider } => {
            tracing::info!(target: "agent", %provider, "declined a question that asks for a secret answer");
        }
        Note::Answering {
            provider,
            approval,
            thread,
        } => {
            tracing::debug!(target: "agent", %provider, %approval, %thread, "answering");
        }
        Note::Untested {
            provider,
            found,
            supported,
        } => {
            tracing::warn!(target: "agent", %provider, %found, %supported, "running an untested version");
        }
        Note::Unversioned { provider } => {
            tracing::warn!(target: "agent", %provider, "running a CLI that did not report its version");
        }
        Note::CredentialExposed { provider } => {
            tracing::warn!(target: "agent", %provider, "the CLI's commands can read the provider credential; install bubblewrap and socat to sandbox them");
        }
    }
}
