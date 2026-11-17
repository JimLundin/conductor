//! The host's private home for Claude Code.

use std::path::PathBuf;

use install::Home;

/// Transcripts outlive Claude Code's default 30-day sweep; Conductor deletes
/// a transcript with its session (§6.2).
const SETTINGS: &str = "{\n  \"cleanupPeriodDays\": 36500\n}\n";

/// The private home at `dir`, with Claude Code's settings on first run.
///
/// # Errors
///
/// Fails if the home or its settings can't be created.
pub fn private_home(dir: PathBuf) -> std::io::Result<Home> {
    Home::private(dir, "settings.json", SETTINGS)
}
