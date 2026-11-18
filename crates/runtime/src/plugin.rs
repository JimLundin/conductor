//! A provider as the host runs it. The host knows each provider only by its
//! [`Plugin`]: the CLI's profile as data, the settings the host was started
//! with, and the two functions only the provider can supply.

use std::path::PathBuf;

use agents::Profile;
use install::Home;
use proto::{Credentials, ModelId};

use crate::Starter;

/// What the host's command line sets for one provider.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// A CLI to run instead of the pinned one, for tests and development.
    pub path: Option<PathBuf>,
    /// The model a session gets when it names none and no credential names
    /// one.
    pub model: Option<String>,
}

/// What a provider's agents start with.
#[derive(Debug, Clone)]
pub struct Launch {
    /// The CLI that passed its probe.
    pub path: PathBuf,
    pub home: Home,
    /// Everything the store holds; each provider reads its own.
    pub credentials: Credentials,
    /// The host's data dir, where a provider keeps what its CLI needs
    /// beside its home.
    pub data_dir: PathBuf,
}

/// The functions are plain `fn`s, not closures or trait objects: a plugin
/// holds no state, and its starter holds whatever its agents share.
#[derive(Debug, Clone)]
pub struct Plugin {
    pub profile: Profile,
    pub settings: Settings,
    /// The default model the stored credentials make, read by the
    /// provider's core.
    pub named: fn(&Credentials) -> Option<ModelId>,
    /// Starts the provider's agents.
    pub starter: fn(Launch) -> Starter,
}
