//! A provider as the host knows it, as data: the provider, how it presents
//! itself and the credentials it takes, the pinned build of its CLI the
//! host installs, the config its private home starts with, and the runs
//! that probe the CLI and sign it in. Each provider's core describes its
//! own (`claude::profile`, `codex::profile`).

use proto::{Provider, ProviderInfo};

use crate::pin::Cli;
use crate::prelude::*;
use crate::{Invocation, SignIn};

/// The file a CLI's private home gets on its first start only, since the
/// CLI edits its own config too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub file: String,
    pub contents: String,
}

/// The runs are for the CLI in the host's private home for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub provider: Provider,
    /// What the host tells the app about the provider.
    pub info: ProviderInfo,
    pub pin: Cli,
    pub config: Config,
    /// The CLI's `--version`, which must succeed for the host to offer it.
    pub version: Invocation,
    /// The CLI's own check, which succeeds when its home is signed in.
    pub status: Invocation,
    /// The CLI's own sign-in, which stores the credential in its home:
    /// from the app, or on the host's terminal (`conductord login`).
    pub sign_in: SignIn,
}
