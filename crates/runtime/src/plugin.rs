//! A provider as the host runs it. The host knows each provider only by its
//! [`Plugin`], which supplies the provider's profile and account adapters.

use std::fmt::Debug;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use agents::{Configuration, Profile};
use agents::{Credential, SignInCode};
use install::{Home, InstallError};
use tokio::sync::oneshot;

use crate::Runtime;

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

/// A provider's description and the instance it builds for each account.
#[derive(Debug, Clone)]
pub struct Plugin {
    pub profile: Profile,
    pub settings: Settings,
    pub instance: fn(Launch) -> Instance,
    /// The account a signed-in home is, from what the CLI reports where
    /// the profile's `account` says.
    pub account: fn(&str) -> Option<String>,
    /// The build of its CLI the host installs when it is given none.
    pub pin: fn() -> Result<agents::pin::Pin, InstallError>,
}
