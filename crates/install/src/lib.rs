//! The plumbing `claude-cli` and `codex-cli` share: the pinned CLIs'
//! installs, each CLI's home, and how its process is started and stopped.

mod home;
mod install;
pub mod line_json;
pub mod process;
pub mod runner;

pub use home::Home;
pub use install::{Cli, InstallError, install, install_fleet};
