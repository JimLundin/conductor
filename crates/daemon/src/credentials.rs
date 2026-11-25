mod adoption;
mod key;

pub use key::{exists as has_file, read_file as read_key};

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use host::registry::{AccountAdoption, AdoptionReplay, Change};
use host::registry::{Catalog, Instance, InstanceId, Roster};
use link::CommandId;
use serde::{Deserialize, Serialize};

use adoption::Adoption;

pub use host::registry::Stored;

const FORMAT: u8 = 9;

#[derive(Default, Serialize, Deserialize)]
struct Credentials {
    roster: Roster,
    removals: BTreeSet<CommandId>,
}

const PURPOSE: &[u8] = b"conductor credentials 1";
const NONCE_LEN: usize = 24;

pub struct Store {
    dir: PathBuf,
    path: PathBuf,
    catalog: Catalog,
    recovered: Option<AccountAdoption>,
    cipher: XChaCha20Poly1305,
}

#[derive(Clone)]
pub struct SigningLease {
    instance: InstanceId,
    pub settings: Instance,
    _lock: Arc<File>,
}

pub struct CredentialLease {
    instance: InstanceId,
    _lock: File,
}

pub struct RecoveredAdoption {
    pub account: AccountAdoption,
    pub roster: Roster,
}

impl Store {
    pub fn open(
        data_dir: &Path,
        catalog: Catalog,
    ) -> anyhow::Result<(Self, Stored)> {
        std::fs::create_dir_all(data_dir)?;
        let lock = credential_lock(data_dir)?;
        let key = key::open(data_dir)?;
        let mut store = Self {
            dir: data_dir.to_path_buf(),
            path: data_dir.join("credentials"),
            catalog,
            recovered: None,
            cipher: XChaCha20Poly1305::new(&Key::from(key)),
        };
        store.recovered =
            store.recover(|_| true)?.map(|adoption| adoption.account);
        let stored = store.read();
        drop(lock);
        Ok((store, stored))
    }

    pub fn signing(
        &self,
        instance: &InstanceId,
    ) -> anyhow::Result<SigningLease> {
        let _lock = self.lock()?;
        self.ensure_idle()?;
        let settings =
            self.read_roster()?.of(instance).cloned().with_context(|| {
                format!("this host has no instance {instance}")
            })?;
        let lock = self.lease(instance)?;
        Ok(SigningLease {
            instance: instance.clone(),
            settings,
            _lock: Arc::new(lock),
        })
    }

    pub fn adopt(
        &self,
        source: &SigningLease,
        account: AccountAdoption,
    ) -> anyhow::Result<Roster> {
        let AccountAdoption { from, to } = &account;
        anyhow::ensure!(
            source.instance == *from,
            "the sign-in no longer has its private home"
        );
        let _lock = self.lock()?;
        self.ensure_idle()?;
        let _destination = self.lease(to)?;
        let roster = self.read_roster()?;
        let next = account.plan(&roster).with_context(|| {
            format!("the source instance {from} cannot be adopted as {to}")
        })?;
        let mut adoption = Adoption::begin(&self.dir, account)?;
        self.complete(&mut adoption, AdoptionReplay::Install(next))
    }

    pub fn claim_credential(
        &self,
        instance: InstanceId,
    ) -> anyhow::Result<CredentialLease> {
        let _lock = self.lock()?;
        self.ensure_idle()?;
        let lock = self.lease(&instance)?;
        Ok(CredentialLease {
            instance,
            _lock: lock,
        })
    }

    pub fn save_change(
        &self,
        planned: &Roster,
        change: &Change,
        claim: Option<&CredentialLease>,
        removal: Option<CommandId>,
    ) -> anyhow::Result<Roster> {
        let _lock = self.lock()?;
        self.ensure_idle()?;
        let changed = match change {
            Change::Credential(instance)
            | Change::Added(instance)
            | Change::Removed(instance) => Some(instance),
            Change::SignedIn(_) => None,
        };
        let _instance = changed
            .filter(|instance| {
                claim.is_none_or(|lease| &lease.instance != *instance)
            })
            .map(|instance| self.lease(instance))
            .transpose()?;
        let mut credentials = self.read_credentials()?.unwrap_or_default();
        let mut roster =
            Stored::Opened(credentials.roster).roster(&self.catalog);
        anyhow::ensure!(
            change.apply(&mut roster, planned),
            "the instance changed before the credential could be stored"
        );
        if let Some(id) = removal {
            credentials.removals.insert(id);
        }
        credentials.roster = roster;
        self.write_credentials(&credentials)?;
        Ok(credentials.roster)
    }

    pub(crate) fn removal(&self, id: CommandId) -> bool {
        self.read_credentials()
            .ok()
            .flatten()
            .is_some_and(|stored| stored.removals.contains(&id))
    }

    fn ensure_idle(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            Adoption::pending(&self.dir)?.is_none(),
            "an earlier account adoption still needs recovery"
        );
        Ok(())
    }

    fn lease(&self, instance: &InstanceId) -> anyhow::Result<File> {
        let locks = self.dir.join("sign-in-locks");
        std::fs::create_dir_all(&locks)?;
        let file = lock_file(&locks.join(host::registry::dir(instance)))?;
        file.try_lock().with_context(|| {
            format!("claiming the private home for {instance}")
        })?;
        Ok(file)
    }

    fn lock(&self) -> anyhow::Result<File> {
        credential_lock(&self.dir)
    }

    pub fn recovered_account(
        &self,
        source: &InstanceId,
    ) -> Option<&InstanceId> {
        self.recovered
            .as_ref()
            .and_then(|adoption| adoption.destination_for(source))
    }

    pub fn resume_adoption(
        &self,
        ready: impl FnOnce(&AccountAdoption) -> bool,
    ) -> anyhow::Result<Option<RecoveredAdoption>> {
        let _lock = self.lock()?;
        self.recover(ready)
    }

    fn recover(
        &self,
        ready: impl FnOnce(&AccountAdoption) -> bool,
    ) -> anyhow::Result<Option<RecoveredAdoption>> {
        let Some(mut adoption) = Adoption::pending(&self.dir)? else {
            return Ok(None);
        };
        anyhow::ensure!(
            ready(&adoption.account),
            "an earlier account adoption is waiting for its sessions to close"
        );
        let _source = self.lease(&adoption.account.from)?;
        let _destination = self.lease(&adoption.account.to)?;
        let replay = adoption
            .account
            .replay(self.read_roster()?)
            .map_err(anyhow::Error::msg)?;
        let roster = self.complete(&mut adoption, replay)?;
        Ok(Some(RecoveredAdoption {
            account: adoption.account,
            roster,
        }))
    }

    fn complete(
        &self,
        adoption: &mut Adoption,
        replay: AdoptionReplay,
    ) -> anyhow::Result<Roster> {
        let roster = match replay {
            AdoptionReplay::Install(next) => {
                adoption.resume_directory_moves(&self.dir)?;
                self.save(&next)?;
                next
            }
            AdoptionReplay::Committed(roster) => {
                crate::sync_dir(&self.dir)?;
                roster
            }
        };
        if let Err(err) = adoption.finish(&self.dir) {
            tracing::warn!(target: "host", reason = %format!("{err:#}"), "could not clean up committed account adoption");
        }
        Ok(roster)
    }

    pub(crate) fn read_roster(&self) -> anyhow::Result<Roster> {
        let stored = self.read();
        if let Stored::Unreadable(reason) = stored {
            anyhow::bail!("{reason}");
        }
        Ok(stored.roster(&self.catalog))
    }

    fn read(&self) -> Stored {
        match self.read_credentials() {
            Ok(Some(credentials)) => Stored::Opened(credentials.roster),
            Ok(None) => Stored::Fresh,
            Err(err) => Stored::Unreadable(format!("{err:#}")),
        }
    }

    fn read_credentials(&self) -> anyhow::Result<Option<Credentials>> {
        match std::fs::read(&self.path) {
            Ok(sealed) => self.unseal(&sealed).map(Some).with_context(|| format!(
                "{} does not open with the store key; clearing a credential moves it aside",
                self.path.display()
            )),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err).with_context(|| format!("reading {}", self.path.display())),
        }
    }

    fn save(&self, roster: &Roster) -> anyhow::Result<()> {
        let mut credentials = self.read_credentials()?.unwrap_or_default();
        credentials.roster = roster.clone();
        self.write_credentials(&credentials)
    }

    fn write_credentials(
        &self,
        credentials: &Credentials,
    ) -> anyhow::Result<()> {
        let mut nonce = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce).context("no random source")?;
        let plain = postcard::to_stdvec(credentials)?;
        let payload = Payload {
            msg: &plain,
            aad: PURPOSE,
        };
        let sealed = self
            .cipher
            .encrypt(XNonce::from_slice(&nonce), payload)
            .ok()
            .context("sealing the credentials failed")?;
        let file = std::iter::once(FORMAT)
            .chain(nonce)
            .chain(sealed)
            .collect::<Vec<u8>>();
        let next = self.path.with_extension("new");
        crate::write_private(&next, &file)
            .with_context(|| format!("writing {}", next.display()))?;
        std::fs::rename(&next, &self.path)
            .with_context(|| format!("replacing {}", self.path.display()))?;
        crate::sync_dir(&self.dir)
            .with_context(|| format!("syncing {}", self.dir.display()))
    }

    pub fn move_aside(&self) -> anyhow::Result<PathBuf> {
        let _lock = self.lock()?;
        self.ensure_idle()?;
        anyhow::ensure!(
            matches!(self.read(), Stored::Unreadable(_)),
            "the store is no longer unreadable"
        );
        let aside = self.path.with_extension("unreadable");
        std::fs::rename(&self.path, &aside)
            .with_context(|| format!("moving {} aside", self.path.display()))?;
        Ok(aside)
    }

    fn unseal(&self, file: &[u8]) -> Option<Credentials> {
        let (format, encrypted) = file.split_first()?;
        let (nonce, ciphertext) = encrypted.split_at_checked(NONCE_LEN)?;
        let payload = Payload {
            msg: ciphertext,
            aad: PURPOSE,
        };
        let plain = self
            .cipher
            .decrypt(XNonce::from_slice(nonce), payload)
            .ok()?;
        match *format {
            FORMAT => postcard::from_bytes(&plain).ok(),
            _ => None,
        }
    }
}

fn credential_lock(dir: &Path) -> anyhow::Result<File> {
    let path = dir.join("credentials.lock");
    let file = lock_file(&path)?;
    file.lock()
        .with_context(|| format!("locking {}", path.display()))?;
    Ok(file)
}

fn lock_file(path: &Path) -> anyhow::Result<File> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))
}

#[cfg(test)]
mod tests;
