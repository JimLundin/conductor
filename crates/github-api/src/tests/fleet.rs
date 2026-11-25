use std::io::Write as _;
use std::{fs, io, path::Path};

use github::access::{AccessError, Repository, TokenKind};
use home::backend::CredentialRef;
use project_runtime::transport::{Method, SharedCredential};
use secrecy::ExposeSecret as _;

use super::selection;
use crate::HostCredentials;

const REFERENCE: &str = "fleet/tracker";
const OAUTH: &str = "repository = 'owner/work'\nexpires_unix = 4102444800\n[source]\nkind = 'oauth'\ntoken = 'fixture'\n";
const OTHER: &str = "repository = 'owner/other'\n[source]\nkind = 'classic'\ntoken = 'another-fixture'\n";

fn resolver(data: &Path, reference: &str) -> io::Result<SharedCredential> {
    assert_eq!(reference, REFERENCE);
    let value = fs::read_to_string(data.join("replicated"))?;
    Ok(match value.as_str() {
        "missing" => SharedCredential::Missing,
        "standalone" => SharedCredential::Standalone,
        _ => SharedCredential::Stored(value.into()),
    })
}

fn local(root: &Path, entry: &str) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let path = root.join("project-credentials.toml");
    fs::write(
        &path,
        format!(
            "[\"{REFERENCE}\"]\n{}\n[\"host/other\"]\n{}",
            entry.replace("[source]", "[\"fleet/tracker\".source]"),
            OTHER.replace("[source]", "[\"host/other\".source]")
        ),
    )?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[tokio::test]
async fn a_fleet_reference_uses_the_shared_record_and_fails_closed_after_revocation()
-> anyhow::Result<()> {
    let data = tempfile::tempdir()?;
    local(data.path(), OAUTH)?;
    let host = HostCredentials::with_fleet(data.path(), resolver);
    let mut selected = selection("owner/work");
    selected.credential = Some(
        CredentialRef::try_from(REFERENCE.to_owned())
            .map_err(anyhow::Error::msg)?,
    );
    let replicated = data.path().join("replicated");
    fs::write(&replicated, OAUTH)?;
    assert_eq!(host.connect(&selected)?.kind, TokenKind::OAuth);
    fs::write(&replicated, OAUTH.replace("'fixture'", "'replacement'"))?;
    let reference = selected
        .credential
        .as_ref()
        .ok_or(AccessError::CredentialMissing)?;
    let git = host
        .git(Repository::parse("owner/work")?, reference)
        .await?;
    assert!(
        matches!(git.method(), Method::Token(value) if value.expose_secret() == "replacement")
    );
    fs::write(&replicated, OTHER)?;
    assert!(matches!(
        host.connect(&selected),
        Err(AccessError::PermissionDenied)
    ));
    fs::write(&replicated, OAUTH.replace("4102444800", "1"))?;
    assert!(matches!(host.connect(&selected), Err(AccessError::Expired)));
    fs::write(
        &replicated,
        OAUTH.replace(
            "kind = 'oauth'\ntoken = 'fixture'",
            "kind = 'ssh_agent'\nhost_key = 'dGVzdA=='",
        ),
    )?;
    assert!(matches!(
        host.connect(&selected),
        Err(AccessError::InvalidConfiguration)
    ));
    fs::write(&replicated, "missing")?;
    assert!(matches!(
        host.connect(&selected),
        Err(AccessError::CredentialMissing)
    ));
    assert!(matches!(
        host.git(Repository::parse("owner/work")?, reference).await,
        Err(AccessError::CredentialMissing)
    ));
    fs::write(&replicated, "malformed")?;
    assert!(matches!(
        host.connect(&selected),
        Err(AccessError::InvalidConfiguration)
    ));
    fs::remove_file(&replicated)?;
    assert!(matches!(
        host.connect(&selected),
        Err(AccessError::CredentialStoreUnavailable)
    ));
    assert!(
        HostCredentials::new(data.path())
            .connect(&selected)
            .is_err()
    );
    fs::write(&replicated, "standalone")?;
    assert_eq!(host.connect(&selected)?.kind, TokenKind::OAuth);
    Ok(())
}

#[test]
fn exporting_a_fleet_credential_validates_one_active_repository_binding()
-> anyhow::Result<()> {
    let data = tempfile::tempdir()?;
    let host = HostCredentials::new(data.path());
    local(data.path(), OAUTH)?;
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(data.path().join("project-credentials.toml"))?;
    writeln!(file, "[unrelated]\nunsupported = true")?;
    let share = host.export_for_fleet(REFERENCE)?;
    let actual = toml::from_str::<toml::Table>(share.expose_secret())
        .map_err(|_| anyhow::anyhow!("exported record is not valid TOML"))?;
    assert!(actual.eq(&toml::from_str::<toml::Table>(OAUTH)?));
    assert!(host.export_for_fleet("host/other").is_err());
    assert!(host.export_for_fleet("fleet/../tracker").is_err());
    for entry in [
        OAUTH.replace("owner/work", "owner/../work"),
        OAUTH.replace("4102444800", "1"),
        OAUTH.replace("'fixture'", "''"),
        OAUTH.replace("'fixture'", r#""bad\ntoken""#),
        OAUTH.replace("'fixture'", "'white space'"),
        OAUTH.replace("kind = 'oauth'\ntoken = 'fixture'", "kind = 'ssh_agent'\nhost_key = 'test'"),
        OAUTH.replace("kind = 'oauth'\ntoken = 'fixture'", "kind = 'installation'\napp_id = 1\ninstallation_id = 2\nprivate_key = 'invalid'"),
    ] {
        local(data.path(), &entry)?;
        assert!(host.export_for_fleet(REFERENCE).is_err());
    }
    Ok(())
}

#[test]
fn exporting_an_installation_requires_a_private_signing_key()
-> anyhow::Result<()> {
    let data = tempfile::tempdir()?;
    let host = HostCredentials::new(data.path());
    let rsa = openssl::rsa::Rsa::generate(2048)?;
    let private = rsa.private_key_to_pem()?;
    local(data.path(), &installation(1, 2, &private)?)?;
    assert!(host.export_for_fleet(REFERENCE).is_ok());
    for (app, id, key) in [
        (0, 2, private.clone()),
        (1, 0, private),
        (1, 2, rsa.public_key_to_pem()?),
    ] {
        local(data.path(), &installation(app, id, &key)?)?;
        assert!(host.export_for_fleet(REFERENCE).is_err());
    }
    Ok(())
}

fn installation(
    app: u64,
    installation: u64,
    key: &[u8],
) -> anyhow::Result<String> {
    let pem = std::str::from_utf8(key)?;
    Ok(format!(
        "repository = 'owner/work'\n[source]\nkind = 'installation'\napp_id = {app}\ninstallation_id = {installation}\nprivate_key = '''{pem}'''\n"
    ))
}
