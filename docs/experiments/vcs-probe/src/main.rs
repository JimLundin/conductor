mod fixture;
mod integration;
mod recovery;
mod snapshot;
mod worktree;

use std::{
    fs,
    io::{self, Write},
};

use anyhow::Result;

fn main() -> Result<()> {
    let scratch =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/probe");
    fs::create_dir_all(&scratch)?;
    let directory = tempfile::tempdir_in(scratch)?;
    snapshot::run(directory.path())?;
    writeln!(
        io::stdout(),
        "PASS snapshot: HEAD/index/files preserved; staging conflict refused"
    )?;
    worktree::run(directory.path())?;
    writeln!(
        io::stdout(),
        "PASS worktree: discovery, ownership refusal, detach, removal"
    )?;
    integration::run(directory.path())?;
    writeln!(
        io::stdout(),
        "PASS integration: diff, clean/conflicting merge, conditional refs, local fetch/push"
    )?;
    recovery::run(directory.path())?;
    writeln!(
        io::stdout(),
        "PASS recovery: reopened boundaries, replay, competing refs, rewind refusal"
    )?;
    Ok(())
}
