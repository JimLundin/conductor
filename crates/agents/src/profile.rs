//! A provider as the host knows it, as data: the provider, how it presents
//! itself and the credentials it takes, the pinned build of its CLI the
//! host installs, the config its private home starts with, and the runs
//! that probe the CLI and sign it in. Each provider's core describes its
//! own (`claude::profile`, `codex::profile`).

use crate::prelude::*;
use crate::{Invocation, Provider, ProviderInfo, SignIn};

/// The file a CLI's private home gets on its first start only, since the
/// CLI edits its own config too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub file: String,
    pub contents: String,
}

/// Where a CLI names the account its home is signed in as, once it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reported {
    /// In what its sign-in check prints.
    Status,
    /// In this file of its home.
    File(String),
}

/// The runs are for the CLI in the host's private home for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub provider: Provider,
    /// What the host tells the app about the provider.
    pub info: ProviderInfo,
    pub config: Config,
    /// The instructions file its CLI reads by itself in its working
    /// directory, which a session's instructions then leave out.
    pub instructions_file: String,
    /// The CLI's `--version`, which must succeed for the host to offer it.
    pub version: Invocation,
    /// The CLI's own check, which succeeds when its home is signed in.
    pub status: Invocation,
    /// Where the CLI names the account a sign-in signed its home in as.
    pub account: Reported,
    /// The CLI's own sign-in, which stores the credential in its home:
    /// from the app, or on the host's terminal (`orchestrator login`).
    pub sign_in: SignIn,
}
