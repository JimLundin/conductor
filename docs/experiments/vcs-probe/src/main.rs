mod features;
mod fixture;
mod installation;
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
    installation::run(directory.path())?;
    writeln!(
        io::stdout(),
        "PASS installation: retained candidate, index/ref boundaries, source preservation, external changes and stale locks"
    )?;
    features::run(directory.path())?;
    writeln!(
        io::stdout(),
        "PASS features: text/eol/binary attributes, custom filter gap, blocking cancellation and stale ref locks"
    )?;
    Ok(())
}
