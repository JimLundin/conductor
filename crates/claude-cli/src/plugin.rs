//! Claude Code as a host runs it.

use runtime::{Plugin, Settings};

use crate::agent::instance;

/// Claude, run as `settings` say.
#[must_use]
pub fn plugin(settings: Settings) -> Plugin {
    Plugin {
        profile: claude::profile(),
        settings,
        instance,
        account: claude::account,
        transcripts: crate::transcript::FILES,
    }
}
