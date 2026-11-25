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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    pub path: Option<PathBuf>,
    pub model: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Launch {
    pub path: PathBuf,
    pub home: Home,
    pub work: PathBuf,
}

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

#[derive(Debug, Clone)]
pub struct Plugin {
    pub profile: Profile,
    pub settings: Settings,
    pub instance: fn(Launch) -> Instance,
    pub account: fn(&str) -> Option<String>,
    pub pin: fn() -> Result<agents::pin::Pin, InstallError>,
}
