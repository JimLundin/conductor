//! The host's private home for Codex.

use std::path::PathBuf;

use install::Home;

const CONFIG: &str = "check_for_update_on_startup = false\n";

/// The private home at `dir`, with Codex's config on first run.
///
/// # Errors
///
/// Fails if the home or its config can't be created.
pub fn private_home(dir: PathBuf) -> std::io::Result<Home> {
    Home::private(dir, "config.toml", CONFIG)
}
