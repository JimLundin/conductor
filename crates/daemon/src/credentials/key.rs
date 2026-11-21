use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context, bail, ensure};
use host::secret::Secret;

const LEGACY: &str = "store.key";
const SLOT: &str = "store.key.slot";
const NATIVE: &str = "store.key.native";
const FILE_ONLY: &str = "store.key.file";

type Key = [u8; 32];

enum Read {
    Found(Key),
    Missing,
    #[cfg(any(test, not(any(target_os = "macos", target_os = "windows"))))]
    Unavailable,
}

trait Vault {
    fn read(&self, account: &str) -> anyhow::Result<Read>;
    fn insert(&mut self, account: &str, key: &Key) -> anyhow::Result<()>;
}

pub(super) fn open(dir: &Path) -> anyhow::Result<Key> {
    let active = read_file(dir, NATIVE)?;
    let file_only = exists(dir, FILE_ONLY)?;
    if file_only {
        return open_file(dir, active);
    }
    open_with(dir, &mut platform::Native, active)
}

fn open_file(dir: &Path, active: Option<Key>) -> anyhow::Result<Key> {
    let legacy = read_file(dir, LEGACY)?;
    if let Some(fingerprint) = active {
        let key = legacy.context("restore the original store.key before selecting file storage for this migrated store")?;
        verify_native_key(&key, &fingerprint)?;
        return Ok(key);
    }
    file_key(dir, legacy)
}

fn verify_native_key(key: &Key, fingerprint: &Key) -> anyhow::Result<()> {
    ensure!(
        blake3::hash(key).as_bytes() == fingerprint,
        "the credential store key differs from the migration fingerprint; recover the original key before starting Conductor"
    );
    Ok(())
}

fn open_with(
    dir: &Path,
    vault: &mut impl Vault,
    active: Option<Key>,
) -> anyhow::Result<Key> {
    let legacy = read_file(dir, LEGACY)?;
    let Some(fingerprint) = active else {
        return migrate(dir, vault, legacy);
    };
    open_native(dir, vault, &fingerprint, legacy)
}

fn open_native(
    dir: &Path,
    vault: &impl Vault,
    fingerprint: &Key,
    legacy: Option<Key>,
) -> anyhow::Result<Key> {
    let slot = read_file(dir, SLOT)?.context(
        "the native credential store slot is missing; see credential recovery",
    )?;
    let account = data_encoding::HEXLOWER.encode(&slot);
    let Read::Found(key) = vault.read(&account)? else {
        bail!(
            "the native credential store key is missing or inaccessible; open the original user keystore or recover the original store key"
        );
    };
    verify_native_key(&key, fingerprint)?;
    if let Some(file_key) = legacy {
        ensure!(
            file_key == key,
            "the file and native credential store keys differ; both have been preserved"
        );
        std::fs::remove_file(dir.join(LEGACY))
            .context("removing the migrated store.key")?;
        crate::sync_dir(dir).context("syncing credential key migration")?;
    }
    Ok(key)
}

fn migrate(
    dir: &Path,
    vault: &mut impl Vault,
    legacy: Option<Key>,
) -> anyhow::Result<Key> {
    let known_slot = read_file(dir, SLOT)?;
    let slot = known_slot.map_or_else(crate::random_key, Ok)?;
    let account = data_encoding::HEXLOWER.encode(&slot);
    let key = match vault.read(&account)? {
        #[cfg(any(test, not(any(target_os = "macos", target_os = "windows"))))]
        Read::Unavailable => {
            ensure!(
                known_slot.is_none() || legacy.is_some(),
                "credential key migration is awaiting its original native keystore"
            );
            tracing::warn!(
                "native credential keystore unavailable; using the private store.key file"
            );
            return file_key(dir, legacy);
        }
        Read::Found(key) => {
            ensure!(
                known_slot.is_some(),
                "a new credential key slot is already occupied"
            );
            ensure!(
                legacy.is_none_or(|file_key| file_key == key),
                "the file and native credential store keys differ; both have been preserved"
            );
            key
        }
        Read::Missing => install_native(dir, vault, &slot, known_slot, legacy)?,
    };
    record_marker(dir, NATIVE, blake3::hash(&key).as_bytes())?;
    // Leave the legacy file through the first open so a restart can verify
    // the persisted marker and native entry before deleting the only file copy.
    Ok(key)
}

fn install_native(
    dir: &Path,
    vault: &mut impl Vault,
    slot: &Key,
    known_slot: Option<Key>,
    legacy: Option<Key>,
) -> anyhow::Result<Key> {
    let key = legacy.map_or_else(|| new_key(dir), Ok)?;
    if known_slot.is_none() {
        record_marker(dir, SLOT, slot)?;
    }
    let account = data_encoding::HEXLOWER.encode(slot);
    vault.insert(&account, &key)?;
    ensure!(
        matches!(vault.read(&account)?, Read::Found(saved) if saved == key),
        "the native credential store key could not be verified"
    );
    Ok(key)
}

fn record_marker(dir: &Path, name: &str, value: &Key) -> anyhow::Result<()> {
    let text = data_encoding::HEXLOWER.encode(value);
    let next = dir.join(format!("{name}.new"));
    crate::write_private(&next, text.as_bytes())
        .with_context(|| format!("writing {name}"))?;
    let linked = crate::link_new(&next, &dir.join(name));
    let _ = std::fs::remove_file(next);
    ensure!(
        linked.with_context(|| format!("recording {name}"))?,
        "the credential key migration marker {name} changed"
    );
    crate::sync_dir(dir).context("syncing credential key migration")
}

fn file_key(dir: &Path, file: Option<Key>) -> anyhow::Result<Key> {
    if let Some(key) = file {
        return Ok(key);
    }
    can_create_key(dir)?;
    crate::load_or_create_secret(dir, LEGACY)
}

fn new_key(dir: &Path) -> anyhow::Result<Key> {
    can_create_key(dir)?;
    crate::random_key()
}

fn can_create_key(dir: &Path) -> anyhow::Result<()> {
    ensure!(
        !exists(dir, "credentials")?
            && !exists(dir, "credentials.new")?
            && !exists(dir, "credentials.unreadable")?,
        "credential files exist without a store key; recover the original store key before starting Conductor"
    );
    Ok(())
}

fn exists(dir: &Path, name: &str) -> anyhow::Result<bool> {
    let path = dir.join(name);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => {
            ensure!(
                metadata.is_file(),
                "{} must be a regular file",
                path.display()
            );
            Ok(true)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => {
            Err(error).with_context(|| format!("inspecting {}", path.display()))
        }
    }
}

fn read_file(dir: &Path, name: &str) -> anyhow::Result<Option<Key>> {
    if !exists(dir, name)? {
        return Ok(None);
    }
    let path = dir.join(name);
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    match host::secret::stored(Some(&text)) {
        Secret::Key(key) => Ok(Some(key)),
        Secret::Missing | Secret::Malformed => {
            bail!("{} is not 64 hex digits", path.display())
        }
    }
}

#[cfg(all(
    not(test),
    any(target_os = "macos", target_os = "windows", target_os = "linux")
))]
fn native_key(secret: Vec<u8>) -> anyhow::Result<Read> {
    secret.try_into().map(Read::Found).map_err(|_| {
        anyhow::anyhow!("the native credential store key has the wrong length")
    })
}

#[cfg(all(not(test), target_os = "macos"))]
mod platform {
    use anyhow::anyhow;
    use security_framework::os::macos::keychain::SecKeychain;

    use super::{Key, Read, Vault, native_key};

    const SERVICE: &str = "Conductor credential store";
    const ITEM_NOT_FOUND: i32 = -25300;

    pub(super) struct Native;

    fn login_keychain() -> anyhow::Result<SecKeychain> {
        SecKeychain::default()
            .map_err(|_| anyhow!("the login Keychain is unavailable"))
    }

    impl Vault for Native {
        fn read(&self, account: &str) -> anyhow::Result<Read> {
            match login_keychain()?.find_generic_password(SERVICE, account) {
                Ok((password, _)) => native_key(password.as_ref().to_vec()),
                Err(error) if error.code() == ITEM_NOT_FOUND => {
                    Ok(Read::Missing)
                }
                Err(_) => Err(anyhow!(
                    "could not read the store key from the login Keychain"
                )),
            }
        }

        fn insert(&mut self, account: &str, key: &Key) -> anyhow::Result<()> {
            login_keychain()?
                .add_generic_password(SERVICE, account, key)
                .map_err(|_| {
                    anyhow!(
                        "could not save the store key in the login Keychain"
                    )
                })
        }
    }
}

#[cfg(all(not(test), target_os = "windows"))]
mod platform {
    use std::collections::HashMap;

    use anyhow::anyhow;
    use keyring_core::api::CredentialStoreApi;
    use keyring_core::{Entry, Error};
    use windows_native_keyring_store::Store;

    use super::{Key, Read, Vault, native_key};

    pub(super) struct Native;

    fn entry(account: &str) -> anyhow::Result<Entry> {
        Store::new()
            .and_then(|store| {
                store.build(
                    "Conductor credential store",
                    account,
                    Some(&HashMap::from([("persistence", "Local")])),
                )
            })
            .map_err(|_| {
                anyhow!("the Windows Credential Manager entry is unavailable")
            })
    }

    impl Vault for Native {
        fn read(&self, account: &str) -> anyhow::Result<Read> {
            match entry(account)?.get_secret() {
                Ok(secret) => native_key(secret),
                Err(Error::NoEntry) => Ok(Read::Missing),
                Err(_) => Err(anyhow!(
                    "could not read the Conductor store key from Windows Credential Manager"
                )),
            }
        }

        fn insert(&mut self, account: &str, key: &Key) -> anyhow::Result<()> {
            entry(account)?.set_secret(key)
                .map_err(|_| anyhow!("could not save the Conductor store key in Windows Credential Manager"))
        }
    }
}

#[cfg(all(not(test), target_os = "linux"))]
mod platform {
    use std::collections::HashMap;
    use std::time::Duration;

    use anyhow::anyhow;
    use dbus_secret_service::{Collection, EncryptionType, SecretService};

    use super::{Key, Read, Vault, native_key};

    pub(super) struct Native;

    fn connected() -> Option<SecretService> {
        if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none()
            && !std::env::var_os("XDG_RUNTIME_DIR").is_some_and(|dir| {
                std::path::Path::new(&dir).join("bus").exists()
            })
        {
            return None;
        }
        SecretService::connect_with_connection_timeout(
            EncryptionType::Dh,
            Duration::from_secs(5),
        )
        .ok()
    }

    fn attributes(account: &str) -> HashMap<&str, &str> {
        HashMap::from([("application", "conductor"), ("store", account)])
    }

    fn writable_collection(service: &SecretService) -> Option<Collection<'_>> {
        let collection = service.get_default_collection().ok()?;
        if service
            .get_collection_by_alias("session")
            .is_ok_and(|session| session.path == collection.path)
            || !collection.is_locked().is_ok_and(|locked| !locked)
        {
            return None;
        }
        Some(collection)
    }

    fn secret(service: &SecretService, account: &str) -> anyhow::Result<Read> {
        let items = service.search_items(attributes(account))
            .map_err(|_| anyhow!("could not search Linux Secret Service for the Conductor store key"))?;
        match (items.unlocked.as_slice(), items.locked.as_slice()) {
            ([item], []) => item.get_secret()
                .map_err(|_| anyhow!("could not read the Conductor store key from Linux Secret Service"))
                .and_then(native_key),
            ([], [_]) => Ok(Read::Unavailable),
            ([], []) => Ok(writable_collection(service).map_or(Read::Unavailable, |_| Read::Missing)),
            _ => Err(anyhow!("Linux Secret Service returned multiple Conductor store keys")),
        }
    }

    impl Vault for Native {
        fn read(&self, account: &str) -> anyhow::Result<Read> {
            connected().map_or(Ok(Read::Unavailable), |service| {
                secret(&service, account)
            })
        }

        fn insert(&mut self, account: &str, key: &Key) -> anyhow::Result<()> {
            let service = connected().ok_or_else(|| {
                anyhow!("Linux Secret Service is unavailable")
            })?;
            let collection = writable_collection(&service)
                .ok_or_else(|| anyhow!("Linux Secret Service has no unlocked default persistent collection"))?;
            collection.create_item(
                "Conductor credential store",
                attributes(account),
                key,
                false,
                "application/octet-stream",
            ).map(drop)
                .map_err(|_| anyhow!("could not save the Conductor store key in Linux Secret Service"))
        }
    }
}

#[cfg(any(
    test,
    not(any(target_os = "macos", target_os = "windows", target_os = "linux"))
))]
mod platform {
    use super::{Key, Read, Vault};

    pub(super) struct Native;

    impl Vault for Native {
        fn read(&self, _account: &str) -> anyhow::Result<Read> {
            Ok(Read::Unavailable)
        }

        fn insert(&mut self, _account: &str, _key: &Key) -> anyhow::Result<()> {
            anyhow::bail!("there is no native credential keystore on this host")
        }
    }
}

#[cfg(test)]
mod tests;
