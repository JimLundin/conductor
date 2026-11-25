use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use github::access::{AccessError, Repository};
use home::backend::{CredentialRef, Selection};
use project_runtime::transport::SharedCredential;
use secrecy::{ExposeSecret as _, SecretString};
use serde::{Deserialize, Serialize, Serializer};
use zeroize::Zeroizing;

use crate::Client;

pub type FleetResolver = fn(&Path, &str) -> std::io::Result<SharedCredential>;

pub struct HostCredentials {
    data: PathBuf,
    fleet: Option<FleetResolver>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    repository: String,
    expires_unix: Option<u64>,
    source: Source,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Source {
    FineGrained {
        #[serde(serialize_with = "share_secret")]
        token: SecretString,
    },
    Classic {
        #[serde(serialize_with = "share_secret")]
        token: SecretString,
    },
    #[serde(rename = "oauth")]
    OAuth {
        #[serde(serialize_with = "share_secret")]
        token: SecretString,
    },
    Installation {
        app_id: std::num::NonZeroU64,
        installation_id: std::num::NonZeroU64,
        #[serde(serialize_with = "share_secret")]
        private_key: SecretString,
    },
    SshAgent {
        host_key: String,
    },
}

fn share_secret<S: Serializer>(
    secret: &SecretString,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(secret.expose_secret())
}

impl Entry {
    fn validate_binding(&self) -> Result<(), AccessError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| AccessError::RequestFailed)?
            .as_secs();
        if self.expires_unix.is_some_and(|end| end <= now) {
            return Err(AccessError::Expired);
        }
        Repository::parse(&self.repository).map(drop)
    }

    fn shared(payload: &SecretString) -> Result<Self, AccessError> {
        let entry: Self = toml::from_str(payload.expose_secret())
            .map_err(|_| AccessError::InvalidConfiguration)?;
        if matches!(entry.source, Source::SshAgent { .. }) {
            return Err(AccessError::InvalidConfiguration);
        }
        Ok(entry)
    }
}

impl HostCredentials {
    #[must_use]
    pub fn new(data: &Path) -> Self {
        Self {
            data: data.to_owned(),
            fleet: None,
        }
    }

    #[must_use]
    pub fn with_fleet(data: &Path, resolver: FleetResolver) -> Self {
        Self {
            data: data.to_owned(),
            fleet: Some(resolver),
        }
    }

    pub fn export_for_fleet(
        &self,
        reference: &str,
    ) -> Result<SecretString, AccessError> {
        let key = CredentialRef::try_from(reference.to_owned())
            .map_err(|_| AccessError::InvalidConfiguration)?;
        if !key.is_fleet() {
            return Err(AccessError::InvalidConfiguration);
        }
        let entry = stored(&self.data, key.as_str())?;
        entry.validate_binding()?;
        crate::client::validate(&entry.source)?;
        toml::to_string(&entry)
            .map(SecretString::from)
            .map_err(|_| AccessError::InvalidConfiguration)
    }

    fn entry(&self, reference: &CredentialRef) -> Result<Entry, AccessError> {
        if !reference.is_fleet() {
            return stored(&self.data, reference.as_str());
        }
        let resolver =
            self.fleet.ok_or(AccessError::CredentialStoreUnavailable)?;
        match resolver(&self.data, reference.as_str())
            .map_err(|_| AccessError::CredentialStoreUnavailable)?
        {
            SharedCredential::Standalone => {
                stored(&self.data, reference.as_str())
            }
            SharedCredential::Missing => Err(AccessError::CredentialMissing),
            SharedCredential::Stored(payload) => Entry::shared(&payload),
        }
    }

    fn resolve(
        &self,
        repo: &Repository,
        reference: Option<&CredentialRef>,
    ) -> Result<Source, AccessError> {
        let key = reference.ok_or(AccessError::CredentialMissing)?;
        let entry = self.entry(key)?;
        entry.validate_binding()?;
        if !repo.matches(&entry.repository) {
            return Err(AccessError::PermissionDenied);
        }
        Ok(entry.source)
    }

    pub fn connect(&self, selected: &Selection) -> Result<Client, AccessError> {
        if selected.adapter != github::ADAPTER {
            return Err(AccessError::InvalidConfiguration);
        }
        let repository = Repository::parse(&selected.resource)?;
        let source = self.resolve(&repository, selected.credential.as_ref())?;
        Client::new(repository, &source, github::mapping::AUTHORITY)
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
        Client::new(repository, &source, github::mapping::AUTHORITY)?
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
    let mut entries = toml::de::DeTable::parse(&contents)
        .map_err(|_| AccessError::InvalidConfiguration)?;
    let entry = entries
        .get_mut()
        .remove(key)
        .ok_or(AccessError::CredentialMissing)?;
    Entry::deserialize(toml::de::ValueDeserializer::from(entry))
        .map_err(|_| AccessError::InvalidConfiguration)
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
