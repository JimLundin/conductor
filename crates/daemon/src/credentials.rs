//! The host's credential store (§6.3): `<data>/credentials` holds the
//! postcard of [`Credentials`], sealed with XChaCha20-Poly1305 under the
//! 32-byte store key in `<data>/store.key`. The key sits beside the store,
//! readable by the owner only, §6.3's variant for hosts without a platform
//! keystore. [`Credentials`] is what the fleet config will carry, so
//! sharing it is a copy of this value.
//!
//! The file is a format byte, the nonce and the ciphertext, sealed with the
//! format and the file's purpose as associated data, so no other sealed
//! blob under the same key opens as credentials.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::Context;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use proto::Credentials;

const FORMAT: u8 = 1;
const PURPOSE: &[u8] = b"conductor credentials 1";
const NONCE_LEN: usize = 24;

pub struct Store {
    dir: PathBuf,
    path: PathBuf,
    cipher: XChaCha20Poly1305,
}

/// What the store held when the host started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stored {
    Opened(Credentials),
    /// The file exists and doesn't open, for this reason. The host runs
    /// without either provider until a Clear moves it aside.
    Unreadable(String),
}

impl Store {
    /// Opens the store in `data_dir`, creating its key on first run, and
    /// reads what it holds. Only a store key that can't be read or made
    /// fails it.
    pub fn open(data_dir: &Path) -> anyhow::Result<(Store, Stored)> {
        let key = crate::load_or_create_secret(data_dir, "store.key")?;
        let store = Store {
            dir: data_dir.to_path_buf(),
            path: data_dir.join("credentials"),
            cipher: XChaCha20Poly1305::new(&Key::from(key)),
        };
        let stored = match std::fs::read(&store.path) {
            Ok(sealed) => store.unseal(&sealed).map_or_else(
                || {
                    Stored::Unreadable(format!(
                        "{} does not open with the store key; clearing a credential moves it aside",
                        store.path.display()
                    ))
                },
                Stored::Opened,
            ),
            Err(err) => match err.kind() {
                ErrorKind::NotFound => Stored::Opened(Credentials::none()),
                _ => Stored::Unreadable(format!("reading {}: {err}", store.path.display())),
            },
        };
        Ok((store, stored))
    }

    /// Replaces what the store holds. It writes and syncs a new file, then
    /// renames it over the old one, so a crash leaves either.
    pub fn save(&self, credentials: &Credentials) -> anyhow::Result<()> {
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
        crate::sync_dir(&self.dir).with_context(|| format!("syncing {}", self.dir.display()))
    }

    /// Moves a store that doesn't open to `credentials.unreadable`,
    /// replacing an older one there, and returns where it went.
    pub fn move_aside(&self) -> anyhow::Result<PathBuf> {
        let aside = self.path.with_extension("unreadable");
        std::fs::rename(&self.path, &aside)
            .with_context(|| format!("moving {} aside", self.path.display()))?;
        Ok(aside)
    }

    fn unseal(&self, file: &[u8]) -> Option<Credentials> {
        let (nonce, ciphertext) = file.strip_prefix(&[FORMAT])?.split_at_checked(NONCE_LEN)?;
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
