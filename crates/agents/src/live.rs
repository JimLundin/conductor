//! Scaffolding for the `s1` and `s2` live-check binaries: scratch setup,
//! fixture log paths and PASS/FAIL reporting. It is public only because
//! each binary is its own crate. It is not part of the adapter API.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;

#[derive(Debug, Clone, Copy)]
pub enum Presence {
    Present,
    Missing,
}
#[must_use]
pub fn presence(path: &Path) -> Presence {
    match std::fs::metadata(path) {
        Ok(_) => Presence::Present,
        Err(_) => Presence::Missing,
    }
}
/// When a live step interrupts its turn.
#[derive(Debug, Clone, Copy)]
pub enum Cue {
    /// Right after sending the message, before the CLI has started the turn.
    Send,
    /// When the turn starts, before the model has produced anything.
    Start,
    /// On the first streamed delta, while the model is producing text.
    Delta,
    /// When the CLI asks for approval, leaving the request unanswered.
    Approval,
}
#[derive(Clone, Copy)]
pub enum Verdict {
    Pass,
    Fail,
}
/// # Errors
/// Fails when stdout cannot be written.
pub fn report(step: &str, verdict: Verdict, detail: &str) -> Result<Verdict> {
    let label = match verdict {
        Verdict::Pass => "PASS",
        Verdict::Fail => "FAIL",
    };
    writeln!(std::io::stdout(), "{label} {step}: {detail}")?;
    Ok(verdict)
}
#[must_use]
pub fn log_path(fixtures: Option<&Path>, name: &str) -> Option<PathBuf> {
    let path = fixtures?.join(format!("{name}.jsonl"));
    let _ = std::fs::remove_file(&path);
    Some(path)
}
/// # Errors
/// Fails when the directory cannot be recreated or `git init` fails.
pub fn fresh_scratch(scratch: &Path) -> Result<()> {
    if matches!(presence(scratch), Presence::Present) {
        std::fs::remove_dir_all(scratch)?;
    }
    std::fs::create_dir_all(scratch)?;
    let git = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(scratch)
        .status()?;
    match git.code() {
        Some(0) => Ok(()),
        _ => Err(anyhow::Error::msg(format!(
            "git init failed in {}",
            scratch.display()
        ))),
    }
}
