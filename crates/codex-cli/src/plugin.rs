use std::sync::Arc;

use runtime::{Instance, Launch, Plugin, Settings};

use super::{hub::Hub, server::ServerOptions};

#[must_use]
pub fn plugin(settings: Settings) -> Plugin {
    Plugin {
        profile: codex::profile(),
        settings,
        instance,
        account: codex::account,
        pin: || install::here(codex::pin::pin),
    }
}

fn instance(launch: Launch) -> Instance {
    Arc::new(Hub::spawn(ServerOptions {
        codex_path: launch.path,
        home: launch.home,
        credential: None,
        cwd: launch.work,
    }))
}
