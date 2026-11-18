//! Codex as a host runs it.

use runtime::{Launch, Plugin, Settings, Starter};

use crate::{Hub, ServerOptions, starter};

/// Codex, run as `settings` say.
#[must_use]
pub fn plugin(settings: Settings) -> Plugin {
    Plugin {
        profile: codex::profile(),
        settings,
        named: codex::runtime::named,
        starter: launched,
    }
}

/// The app-server's own working directory is the data dir; each thread
/// runs in its session's directory.
fn launched(launch: Launch) -> Starter {
    starter(Hub::spawn(ServerOptions {
        codex_path: launch.path,
        home: launch.home,
        credential: codex::runtime::credential(&launch.credentials),
        cwd: launch.data_dir,
        stdout_log: None,
    }))
}
