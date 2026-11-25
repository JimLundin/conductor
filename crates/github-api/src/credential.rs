#[cfg(unix)]
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use github::access::{AccessError, Repository};
use home::backend::{CredentialRef, Selection};
use secrecy::{ExposeSecret as _, SecretString};
use serde::Deserialize;
use zeroize::Zeroizing;

use crate::Client;

pub struct HostCredentials {
    data: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    repository: String,
    expires_unix: Option<u64>,
    source: Source,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Source {
    FineGrained {
        token: SecretString,
    },
    Classic {
        token: SecretString,
    },
    Installation {
        app_id: u64,
        installation_id: u64,
        private_key: SecretString,
    },
    SshAgent {
        host_key: String,
    },
}

impl HostCredentials {
    #[must_use]
    pub fn new(data: &Path) -> Self {
        Self {
            data: data.to_owned(),
        }
    }

    fn resolve(
        &self,
        repo: &Repository,
        reference: Option<&CredentialRef>,
    ) -> Result<Source, AccessError> {
        let key = reference.ok_or(AccessError::CredentialMissing)?.as_str();
        let entry = stored(&self.data, key)?;
        if !repo.matches(&entry.repository) {
            return Err(AccessError::PermissionDenied);
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| AccessError::RequestFailed)?
            .as_secs();
        if entry.expires_unix.is_some_and(|end| end <= now) {
            return Err(AccessError::Expired);
        }
        Ok(entry.source)
    }

    pub fn connect(&self, selected: &Selection) -> Result<Client, AccessError> {
        if selected.adapter != github::ADAPTER {
            return Err(AccessError::InvalidConfiguration);
        }
        let repository = Repository::parse(&selected.resource)?;
        let source = self.resolve(&repository, selected.credential.as_ref())?;
        Client::new(repository, source, github::mapping::AUTHORITY)
    }

    pub async fn git(
        &self,
        repository: Repository,
        reference: &CredentialRef,
    ) -> Result<project_runtime::transport::Credentials, AccessError> {
        let source = self.resolve(&repository, Some(reference))?;
        if let Source::SshAgent { host_key } = source {
            return ssh(&repository, &host_key);
        }
        Client::new(repository, source, github::mapping::AUTHORITY)?
            .git()
            .await
    }
}

fn ssh(
    repo: &Repository,
    host_key: &str,
) -> Result<project_runtime::transport::Credentials, AccessError> {
    let key = data_encoding::BASE64
        .decode(host_key.as_bytes())
        .map_err(|_| AccessError::InvalidConfiguration)?;
    if key.is_empty() {
        return Err(AccessError::InvalidConfiguration);
    }
    Ok(project_runtime::transport::Credentials::ssh(
        format!("ssh://git@github.com/{}/{}.git", repo.owner(), repo.name()),
        String::from("git"),
        String::from("github.com"),
        key,
    ))
}

#[cfg(unix)]
fn stored(data: &Path, key: &str) -> Result<Entry, AccessError> {
    use std::io::Read as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(data.join("project-credentials.toml"))
        .map_err(|error| read_error(error.kind()))?;
    let metadata = file.metadata().map_err(|error| read_error(error.kind()))?;
    if !private_file(&metadata) {
        return Err(AccessError::CredentialStoreUnavailable);
    }
    let mut contents = Zeroizing::new(String::new());
    file.take(1_048_577)
        .read_to_string(&mut contents)
        .map_err(|error| read_error(error.kind()))?;
    if contents.len() > 1_048_576 {
        return Err(AccessError::CredentialStoreUnavailable);
    }
    let mut entries: BTreeMap<String, Entry> = toml::from_str(&contents)
        .map_err(|_| AccessError::InvalidConfiguration)?;
    entries.remove(key).ok_or(AccessError::CredentialMissing)
}

#[cfg(unix)]
fn private_file(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    meta.is_file()
        && meta.mode().trailing_zeros() >= 6
        && meta.uid() == rustix::process::geteuid().as_raw()
}

#[cfg(unix)]
fn read_error(error: std::io::ErrorKind) -> AccessError {
    if error == std::io::ErrorKind::NotFound {
        AccessError::CredentialMissing
    } else {
        AccessError::CredentialStoreUnavailable
    }
}

#[cfg(windows)]
fn stored(data: &Path, key: &str) -> Result<Entry, AccessError> {
    use keyring_core::api::CredentialStoreApi as _;
    use windows_native_keyring_store::Store;
    let canonical = std::fs::canonicalize(data)
        .map_err(|_| AccessError::CredentialStoreUnavailable)?;
    let account = format!("{}|{key}", canonical.display());
    let bytes = Store::new()
        .and_then(|store| {
            store.build("Conductor project credential", &account, None)
        })
        .and_then(|entry| entry.get_secret())
        .map_err(|error| match error {
            keyring_core::Error::NoEntry => AccessError::CredentialMissing,
            _ => AccessError::CredentialStoreUnavailable,
        })?;
    let secret = Zeroizing::new(bytes);
    let text = std::str::from_utf8(&secret)
        .map_err(|_| AccessError::InvalidConfiguration)?;
    toml::from_str(text).map_err(|_| AccessError::InvalidConfiguration)
}

pub fn token(value: SecretString) -> Result<SecretString, AccessError> {
    if value.expose_secret().is_empty() {
        Err(AccessError::InvalidConfiguration)
    } else {
        Ok(value)
    }
}
