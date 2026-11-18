//! Claude Code as a host runs it.

use runtime::{Launch, Plugin, Settings, Starter};

use crate::{ClaudeLaunch, starter};

/// Claude, run as `settings` say. A Claude credential names no model.
#[must_use]
pub fn plugin(settings: Settings) -> Plugin {
    Plugin {
        profile: claude::profile(),
        settings,
        named: |_| None,
        starter: launched,
    }
}

/// Claude lists its models in `<data>/probe`.
fn launched(launch: Launch) -> Starter {
    starter(ClaudeLaunch {
        claude_path: launch.path,
        home: launch.home,
        credential: claude::runtime::credential(&launch.credentials),
        probe: launch.data_dir.join("probe"),
    })
}
