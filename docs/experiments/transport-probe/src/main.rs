mod api;
mod git;
mod server;
mod ssh;

use std::path::PathBuf;

use anyhow::{Context, Result};

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::args().nth(1).as_deref() == Some("ssh-wire") {
        return ssh::wire();
    }
    let root = PathBuf::from(
        std::env::var_os("TRANSPORT_PROBE_FIXTURE")
            .context("run through run.sh")?,
    );
    let workspace = tempfile::tempdir_in(&root)?;
    if std::env::args().nth(1).as_deref() == Some("ssh-missing-agent") {
        return ssh::missing_agent(&root, workspace.path());
    }
    let version = git2::Version::get();
    anyhow::ensure!(
        version.libgit2_version() == (1, 9, 7),
        "unexpected libgit2 version"
    );
    anyhow::ensure!(version.https(), "HTTPS transport not compiled");
    anyhow::ensure!(version.ssh(), "SSH transport not compiled");
    git::cases(&root, workspace.path())?;
    api::cases(&root, workspace.path()).await?;
    ssh::cases(&root, workspace.path())
}
