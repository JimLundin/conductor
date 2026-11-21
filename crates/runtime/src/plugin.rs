//! A provider as the host runs it. The host knows each provider only by its
//! [`Plugin`], which supplies the provider's profile and account adapters.

use std::fmt::Debug;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use agents::{Configuration, Profile};
use install::Home;
use proto::{Credential, SessionId, SignInCode};
use tokio::sync::oneshot;

use crate::{Runtime, Transcript};

/// What the host's command line sets for one provider: the defaults of
/// each of its instances.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// A CLI to run instead of the pinned one, for tests and development.
    pub path: Option<PathBuf>,
    /// The model a session gets when it names none and no credential names
    /// one.
    pub model: Option<String>,
}

/// The CLI and private directories of one provider account.
#[derive(Debug, Clone)]
pub struct Launch {
    pub path: PathBuf,
    pub home: Home,
    pub work: PathBuf,
    pub fleet_flags: Option<proto::fleet::Flags>,
}

/// The outer error is a credential refusal. A configured instance reports
/// model discovery separately in its configuration.
pub type Configuring<'a> =
    Pin<Box<dyn Future<Output = Result<Configuration, String>> + Send + 'a>>;

pub type CredentialSigningIn<'a> =
    Pin<Box<dyn Future<Output = Result<Credential, String>> + Send + 'a>>;

pub trait ProviderInstance: Send + Sync + Debug {
    fn configure(&self, credential: Option<Credential>) -> Configuring<'_>;
    fn start(self: Arc<Self>) -> Runtime;

    fn credential_sign_in(
        &self,
        _kind: &str,
        _account_label: String,
        _link: oneshot::Sender<String>,
        _code: oneshot::Receiver<SignInCode>,
    ) -> Option<CredentialSigningIn<'_>> {
        None
    }
}

pub type Instance = Arc<dyn ProviderInstance>;

type ReadTranscript =
    fn(&Path, SessionId, &[String]) -> Result<Transcript, String>;
type WriteTranscript =
    fn(&Path, SessionId, &[String], &Transcript) -> Result<(), String>;
type RemoveTranscript = fn(&Path, SessionId, &[String]) -> Result<(), String>;

/// Portable session history, with provider-specific lookup and validation.
///
/// The caller supplies a private home and suspends the session before export.
/// Remove is called only after the destination has durably accepted ownership.
#[derive(Debug, Clone, Copy)]
pub struct TranscriptFiles {
    pub export: ReadTranscript,
    pub import: WriteTranscript,
    pub remove: RemoveTranscript,
}

/// A provider's description and the instance it builds for each account.
#[derive(Debug, Clone)]
pub struct Plugin {
    pub profile: Profile,
    pub settings: Settings,
    pub instance: fn(Launch) -> Instance,
    /// The account a signed-in home is, from what the CLI reports where
    /// the profile's `account` says.
    pub account: fn(&str) -> Option<String>,
    pub transcripts: TranscriptFiles,
}
