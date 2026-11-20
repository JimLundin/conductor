//! Codex as a host runs it.

use std::sync::Arc;

use runtime::{Instance, Launch, Plugin, Settings};

use crate::{Hub, ServerOptions};

/// Codex, run as `settings` say.
#[must_use]
pub fn plugin(settings: Settings) -> Plugin {
    Plugin {
        profile: codex::profile(),
        settings,
        instance,
        account: codex::account,
    }
}

/// Each instance gets a hub of its own, so its own app-server, whose
/// environment holds the instance's key. The server's working directory
/// is the instance's work dir; each thread runs in its session's
/// directory.
fn instance(launch: Launch) -> Instance {
    Arc::new(Hub::spawn(ServerOptions {
        codex_path: launch.path,
        home: launch.home,
        credential: None,
        cwd: launch.work,
        stdout_log: None,
    }))
}
