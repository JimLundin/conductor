//! The host's store of its instances and their credentials (§6.3).
//!
//! `<data>/credentials` holds the
//! postcard of [`Roster`], sealed with XChaCha20-Poly1305 under the
//! 32-byte store key in `<data>/store.key`. The key sits beside the store,
//! readable by the owner only, §6.3's variant for hosts without a platform
//! keystore. [`Roster`] is what the fleet config will carry, so sharing it
//! is a copy of this value.
//!
//! The file is a format byte, the nonce and the ciphertext, sealed with the
//! format and the file's purpose as associated data, so no other sealed
//! blob under the same key opens as credentials.

mod adoption;

use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use host::registry::{Change, signed_in_as};
use proto::{Catalog, InstanceId, Roster};

use adoption::Adoption;

pub use host::registry::Stored;

/// Format 5 holds the host's instances, each by its provider and account
/// with its credential as its form's values; an older store reads as one
/// that doesn't open, which removing an instance moves aside.
const FORMAT: u8 = 5;
const PURPOSE: &[u8] = b"conductor credentials 1";
const NONCE_LEN: usize = 24;

pub struct Store {
    dir: PathBuf,
    path: PathBuf,
    catalog: Catalog,
    recovered: Option<(InstanceId, InstanceId)>,
    cipher: XChaCha20Poly1305,
}

#[derive(Clone)]
pub struct SigningLease {
    instance: InstanceId,
    _lock: Arc<File>,
}

pub struct RecoveredAdoption {
    pub from: InstanceId,
    pub to: InstanceId,
    pub roster: Roster,
}

impl Store {
    /// The data dir the store is in.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Opens the store in `data_dir`, creating its key on first run, and
    /// reads what it holds.
    ///
    /// # Errors
    ///
    /// Fails if the key cannot be opened or an unfinished adoption cannot
    /// be recovered.
    pub fn open(
        data_dir: &Path,
        catalog: Catalog,
    ) -> anyhow::Result<(Self, Stored)> {
        let key = crate::load_or_create_secret(data_dir, "store.key")?;
        let mut store = Self {
            dir: data_dir.to_path_buf(),
            path: data_dir.join("credentials"),
            catalog,
            recovered: None,
            cipher: XChaCha20Poly1305::new(&Key::from(key)),
        };
        let lock = store.lock()?;
        store.recovered = store
            .recover()?
            .map(|adoption| (adoption.from, adoption.to));
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
        anyhow::ensure!(
            self.read().roster(&self.catalog).of(instance).is_some(),
            "this host has no instance {instance}"
        );
        self.lease(instance)
    }

    pub fn adopt(
        &self,
        source: &SigningLease,
        to: &InstanceId,
    ) -> anyhow::Result<Roster> {
        let from = &source.instance;
        let _lock = self.lock()?;
        self.ensure_idle()?;
        let _destination = self.lease(to)?;
        let roster = self.read_roster()?;
        anyhow::ensure!(
            from.provider == to.provider
                && signed_in_as(&roster, from, Some(to.account.clone()))
                    .is_some(),
            "the source instance {from} cannot be adopted as {to}"
        );
        let adoption = Adoption::begin(&self.dir, from, to)?;
        self.complete(&adoption, roster)
    }

    pub fn save_change(
        &self,
        planned: &Roster,
        change: &Change,
    ) -> anyhow::Result<Roster> {
        let _lock = self.lock()?;
        self.ensure_idle()?;
        let _instance = match change {
            Change::Removed(instance) => Some(self.lease(instance)?),
            _ => None,
        };
        let mut roster = self.read_roster()?;
        anyhow::ensure!(
            change.apply(&mut roster, planned),
            "the instance changed before the credential could be stored"
        );
        self.save(&roster)?;
        Ok(roster)
    }

    fn ensure_idle(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            Adoption::pending(&self.dir)?.is_none(),
            "an earlier account adoption still needs recovery"
        );
        Ok(())
    }

    fn lease(&self, instance: &InstanceId) -> anyhow::Result<SigningLease> {
        let locks = self.dir.join("sign-in-locks");
        std::fs::create_dir_all(&locks)?;
        let file = lock_file(&locks.join(host::registry::dir(instance)))?;
        file.try_lock().with_context(|| {
            format!("claiming the private home for {instance}")
        })?;
        Ok(SigningLease {
            instance: instance.clone(),
            _lock: Arc::new(file),
        })
    }

    fn lock(&self) -> anyhow::Result<File> {
        let path = self.dir.join("credentials.lock");
        let file = lock_file(&path)?;
        file.lock()
            .with_context(|| format!("locking {}", path.display()))?;
        Ok(file)
    }

    pub fn recovered_account(
        &self,
        source: &InstanceId,
    ) -> Option<&InstanceId> {
        self.recovered
            .as_ref()
            .filter(|(from, _)| from == source)
            .map(|(_, to)| to)
    }

    pub fn resume_adoption(&self) -> anyhow::Result<Option<RecoveredAdoption>> {
        let _lock = self.lock()?;
        self.recover()
    }

    fn recover(&self) -> anyhow::Result<Option<RecoveredAdoption>> {
        let Some(adoption) = Adoption::pending(&self.dir)? else {
            return Ok(None);
        };
        let _source = self.lease(&adoption.from)?;
        let _destination = self.lease(&adoption.to)?;
        let roster = self.complete(&adoption, self.read_roster()?)?;
        let Adoption { from, to, .. } = adoption;
        Ok(Some(RecoveredAdoption { from, to, roster }))
    }

    fn complete(
        &self,
        adoption: &Adoption,
        roster: Roster,
    ) -> anyhow::Result<Roster> {
        let next = signed_in_as(
            &roster,
            &adoption.from,
            Some(adoption.to.account.clone()),
        )
        .map(|(next, _)| next);
        if let Some(updated) = &next {
            adoption.move_directories(&self.dir)?;
            self.save(updated)?;
        } else {
            anyhow::ensure!(
                roster.of(&adoption.to).is_some(),
                "the adoption has neither a source nor a destination instance"
            );
            crate::sync_dir(&self.dir)?;
        }
        adoption.finish(&self.dir)?;
        Ok(next.unwrap_or(roster))
    }

    fn read_roster(&self) -> anyhow::Result<Roster> {
        let stored = self.read();
        if let Stored::Unreadable(reason) = stored {
            anyhow::bail!("{reason}");
        }
        Ok(stored.roster(&self.catalog))
    }

    fn read(&self) -> Stored {
        match std::fs::read(&self.path) {
            Ok(sealed) => self.unseal(&sealed).map_or_else(
                || {
                    Stored::Unreadable(format!(
                        "{} does not open with the store key; clearing a credential moves it aside",
                        self.path.display()
                    ))
                },
                Stored::Opened,
            ),
            Err(err) => match err.kind() {
                ErrorKind::NotFound => Stored::Fresh,
                _ => Stored::Unreadable(format!("reading {}: {err}", self.path.display())),
            },
        }
    }

    /// Replaces what the store holds. It writes and syncs a new file, then
    /// renames it over the old one, so a crash leaves either.
    ///
    /// # Errors
    ///
    /// Fails if there is no random source, or writing, renaming or syncing
    /// the file fails.
    fn save(&self, roster: &Roster) -> anyhow::Result<()> {
        let mut nonce = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce).context("no random source")?;
        let plain = postcard::to_stdvec(roster)?;
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

    /// Moves a store that doesn't open to `credentials.unreadable`,
    /// replacing an older one there, and returns where it went.
    ///
    /// # Errors
    ///
    /// Fails if the rename fails.
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

    fn unseal(&self, file: &[u8]) -> Option<Roster> {
        let (nonce, ciphertext) =
            file.strip_prefix(&[FORMAT])?.split_at_checked(NONCE_LEN)?;
        let payload = Payload {
            msg: ciphertext,
            aad: PURPOSE,
        };
        let plain = self
            .cipher
            .decrypt(XNonce::from_slice(nonce), payload)
            .ok()?;
        postcard::from_bytes(&plain).ok()
    }
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
