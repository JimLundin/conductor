use agents::pin::Pin;
use install::InstallError;
use runtime::{Plugin, Settings};

use crate::agent::instance;

#[must_use]
pub fn plugin(settings: Settings) -> Plugin {
    Plugin {
        profile: claude::profile(),
        settings,
        instance,
        account: claude::account,
        pin,
    }
}

fn pin() -> Result<Pin, InstallError> {
    if cfg!(target_os = "linux")
        && !cfg!(any(target_env = "gnu", target_env = "musl"))
    {
        return Err(InstallError::Unsupported(String::from(
            "Claude has no pinned Linux build for this libc; supported: GNU libc and musl",
        )));
    }
    install::here(claude::pin::pin)
}
