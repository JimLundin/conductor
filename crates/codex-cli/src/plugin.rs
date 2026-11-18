//! Codex as a host runs it.

use runtime::{Launch, Plugin, Settings, Starter};

use crate::{Hub, ServerOptions, starter};

/// Codex, run as `settings` say.
#[must_use]
pub fn plugin(settings: Settings) -> Plugin {
    Plugin {
        profile: codex::profile(),
        settings,
        starter: launched,
        account: codex::account,
    }
}

/// Each instance gets a hub of its own, so its own app-server, whose
/// environment holds the instance's key. The server's working directory
/// is the instance's work dir; each thread runs in its session's
/// directory.
fn launched(launch: Launch) -> Starter {
    starter(Hub::spawn(ServerOptions {
        codex_path: launch.path,
        home: launch.home,
        credential: None,
        cwd: launch.work,
        stdout_log: None,
    }))
}
