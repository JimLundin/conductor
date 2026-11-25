use std::{io::Write, path::Path};

use anyhow::{Context, Result};
use git2::{
    CertificateCheckStatus, Cred, CredentialType, Direction, RemoteCallbacks,
    Repository,
};
use transport_probe_core::{Credential, Transport, credential};

fn key_check(
    cert: &git2::cert::Cert<'_>,
    expected: &[u8],
) -> Result<CertificateCheckStatus, git2::Error> {
    if cert.as_hostkey().and_then(git2::cert::CertHostkey::hostkey)
        == Some(expected)
    {
        Ok(CertificateCheckStatus::CertificateOk)
    } else {
        Err(git2::Error::from_str("fixture SSH host key mismatch"))
    }
}

fn connect(
    repo: &Repository,
    url: &str,
    username: &str,
    expected: &[u8],
) -> Result<(Result<(), git2::Error>, usize)> {
    let mut count = 0;
    let result = {
        let mut callbacks = RemoteCallbacks::new();
        callbacks.certificate_check(|cert, _| key_check(cert, expected));
        callbacks.credentials(|_, _, allowed| {
            count += 1;
            match credential(
                Transport::Ssh,
                allowed.contains(CredentialType::USERNAME),
                allowed.contains(CredentialType::SSH_KEY),
                false,
                count,
            ) {
                Credential::Username => Cred::username(username),
                Credential::Agent => Cred::ssh_key_from_agent(username),
                _ => Err(git2::Error::from_str(
                    "fixture unsupported SSH credential",
                )),
            }
        });
        repo.remote_anonymous(url)?
            .connect_auth(Direction::Push, Some(callbacks), None)
            .map(drop)
    };
    Ok((result, count))
}

pub fn cases(root: &Path, workspace: &Path) -> Result<()> {
    let username = std::env::var("TRANSPORT_PROBE_USER")?;
    let port = std::env::var("TRANSPORT_PROBE_SSH_PORT")?;
    let expected = std::fs::read(root.join("host.pub.bin"))?;
    let repo = Repository::init_bare(workspace.join("ssh-local"))?;
    let url = format!("ssh://{username}@127.0.0.1:{port}/fixture");
    let (valid, callbacks) = connect(&repo, &url, &username, &expected)?;
    valid.context("SSH agent authentication failed")?;
    anyhow::ensure!(callbacks > 0, "SSH credentials callback not called");
    let (without_username, _) = connect(
        &repo,
        &format!("ssh://127.0.0.1:{port}/fixture"),
        &username,
        &expected,
    )?;
    without_username.context("configured SSH username callback failed")?;
    let (wrong_key, _) = connect(&repo, &url, &username, b"wrong host key")?;
    anyhow::ensure!(wrong_key.is_err(), "SSH host key mismatch accepted");
    std::io::stdout().write_all(b"SSH: disposable agent authentication and pinned host-key refusal passed\n")?;
    Ok(())
}

pub fn missing_agent(root: &Path, workspace: &Path) -> Result<()> {
    let username = std::env::var("TRANSPORT_PROBE_USER")?;
    let port = std::env::var("TRANSPORT_PROBE_SSH_PORT")?;
    let expected = std::fs::read(root.join("host.pub.bin"))?;
    let repo = Repository::init_bare(workspace.join("ssh-missing-agent"))?;
    let (result, calls) = connect(
        &repo,
        &format!("ssh://{username}@127.0.0.1:{port}/fixture"),
        &username,
        &expected,
    )?;
    anyhow::ensure!(result.is_err(), "unavailable agent credential accepted");
    anyhow::ensure!(calls > 0, "SSH credentials were not requested");
    anyhow::ensure!(calls <= 3, "SSH credential callback unbounded");
    std::io::stdout()
        .write_all(b"SSH: unavailable agent credential refused\n")?;
    Ok(())
}

pub fn wire() -> Result<()> {
    std::io::stdout()
        .write_all(crate::git::empty_advertisement().as_bytes())?;
    std::io::stdout().flush()?;
    let mut buffer = Vec::new();
    std::io::Read::read_to_end(&mut std::io::stdin(), &mut buffer)?;
    Ok(())
}
