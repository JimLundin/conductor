use std::path::Path;

use anyhow::{Context, bail, ensure};
use fleet_store::keys::{exists, read_file};
use host::secret::{StoreFiles, StoreKey};

const FILE: &str = "store.key";
const SLOT: &str = "store.key.slot";
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
    open_with(dir, &mut platform::Native)
}

fn open_with(dir: &Path, vault: &mut impl Vault) -> anyhow::Result<Key> {
    let found = StoreFiles {
        slot: read_file(dir, SLOT)?,
        file: read_file(dir, FILE)?,
        sealed: sealed(dir)?,
        file_only: exists(dir, FILE_ONLY)?,
    };
    match host::secret::store_key(&found) {
        StoreKey::Keystore(slot) => open_native(vault, &slot),
        StoreKey::File(key) => Ok(key),
        StoreKey::NewInKeystore => create(dir, vault),
        StoreKey::NewInFile => crate::load_or_create_secret(dir, FILE),
        StoreKey::Lost => bail!(
            "credential files exist without a store key; restore the original store key before starting Conductor"
        ),
    }
}

fn sealed(dir: &Path) -> anyhow::Result<bool> {
    Ok(exists(dir, "credentials")?
        || exists(dir, "credentials.new")?
        || exists(dir, "credentials.unreadable")?)
}

fn open_native(vault: &impl Vault, slot: &Key) -> anyhow::Result<Key> {
    let Read::Found(key) = vault.read(&account(slot))? else {
        bail!(
            "the native credential store key is missing or inaccessible; open the original user keystore or restore its entry (docs/credential-store.md)"
        );
    };
    Ok(key)
}

fn create(dir: &Path, vault: &mut impl Vault) -> anyhow::Result<Key> {
    let slot = crate::random_key()?;
    let account = account(&slot);
    match vault.read(&account)? {
        #[cfg(any(test, not(any(target_os = "macos", target_os = "windows"))))]
        Read::Unavailable => {
            tracing::warn!(
                "native credential keystore unavailable; using the private store.key file"
            );
            crate::load_or_create_secret(dir, FILE)
        }
        Read::Found(_) => {
            bail!("a new credential key slot is already occupied")
        }
        Read::Missing => {
            let key = crate::random_key()?;
            vault.insert(&account, &key)?;
            ensure!(
                matches!(vault.read(&account)?, Read::Found(saved) if saved == key),
                "the native credential store key could not be verified"
            );
            record_slot(dir, &slot)?;
            Ok(key)
        }
    }
}

fn account(slot: &Key) -> String {
    data_encoding::HEXLOWER.encode(slot)
}

fn record_slot(dir: &Path, slot: &Key) -> anyhow::Result<()> {
    let next = dir.join(format!("{SLOT}.new"));
    crate::write_private(&next, account(slot).as_bytes())
        .with_context(|| format!("writing {SLOT}"))?;
    let linked = crate::link_new(&next, &dir.join(SLOT));
    let _ = std::fs::remove_file(next);
    ensure!(
        linked.with_context(|| format!("recording {SLOT}"))?,
        "{SLOT} appeared while the store was created"
    );
    crate::sync_dir(dir).with_context(|| format!("syncing {SLOT}"))
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
