//! A provider as the host runs it. The host knows each provider only by its
//! [`Plugin`]: data, and the one function only the provider can supply.

use std::path::PathBuf;

use agents::Profile;
use install::Home;

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

/// Where a provider's agents run.
///
/// The data dir is where a provider keeps what its CLI needs beside the
/// home. The agents start with no credential; the host hands them the
/// stored ones with `Command::Credentials`, as it does each change.
#[derive(Debug, Clone)]
pub struct Launch {
    pub path: PathBuf,
    pub home: Home,
    pub data_dir: PathBuf,
}

/// The profile and settings are data; the starter is the provider's
/// runner, which speaks `agents::runtime` and so holds everything else a
/// provider decides, such as what its credentials become.
#[derive(Debug, Clone)]
pub struct Plugin {
    pub profile: Profile,
    pub settings: Settings,
    pub starter: fn(Launch) -> Starter,
}
