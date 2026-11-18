//! Claude Code as a host runs it.

use runtime::{Launch, Plugin, Settings, Starter};

use crate::{ClaudeLaunch, starter};

/// Claude, run as `settings` say.
#[must_use]
pub fn plugin(settings: Settings) -> Plugin {
    Plugin {
        profile: claude::profile(),
        settings,
        starter: launched,
    }
}

/// Claude lists its models in the instance's work dir.
fn launched(launch: Launch) -> Starter {
    starter(ClaudeLaunch {
        claude_path: launch.path,
        home: launch.home,
        credential: None,
        probe: launch.work,
    })
}
