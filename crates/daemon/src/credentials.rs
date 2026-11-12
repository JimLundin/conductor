//! The host's credential store (§6.3): `<data>/credentials` holds the
//! postcard of [`Credentials`], sealed with XChaCha20-Poly1305 under the
//! 32-byte store key in `<data>/store.key`. The key sits beside the store
//! with mode 0600, §6.3's variant for hosts without a platform keystore.
//! [`Credentials`] is what the fleet config will carry, so sharing it is a
//! copy of this value.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::Context;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use proto::Credentials;

const NONCE_LEN: usize = 24;

pub struct Store {
    path: PathBuf,
    cipher: XChaCha20Poly1305,
}

impl Store {
    /// Opens the store in `data_dir`, creating its key on first run, and
    /// reads what it holds.
    pub fn open(data_dir: &Path) -> anyhow::Result<(Store, Credentials)> {
        let key = crate::load_or_create_secret(data_dir, "store.key")?;
        let store = Store {
            path: data_dir.join("credentials"),
            cipher: XChaCha20Poly1305::new(&Key::from(key)),
        };
        let credentials = match std::fs::read(&store.path) {
            Ok(sealed) => store.unseal(&sealed).with_context(|| {
                format!("{} does not open with the store key", store.path.display())
            }),
            Err(err) => match err.kind() {
                ErrorKind::NotFound => Ok(Credentials::none()),
                _ => Err(err).with_context(|| format!("reading {}", store.path.display())),
            },
        }?;
        Ok((store, credentials))
    }

    /// Replaces what the store holds. It writes a new file and renames it
    /// over the old one, so a crash leaves either.
    pub fn save(&self, credentials: &Credentials) -> anyhow::Result<()> {
        let mut nonce = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce).context("no random source")?;
        let sealed = self
            .cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                postcard::to_stdvec(credentials)?.as_slice(),
            )
            .ok()
            .context("sealing the credentials failed")?;
        let next = self.path.with_extension("new");
        let file = nonce.iter().chain(&sealed).copied().collect::<Vec<u8>>();
        crate::write_private(&next, &file)
            .with_context(|| format!("writing {}", next.display()))?;
        std::fs::rename(&next, &self.path)
            .with_context(|| format!("replacing {}", self.path.display()))
    }

    fn unseal(&self, sealed: &[u8]) -> Option<Credentials> {
        let (nonce, ciphertext) = sealed.split_at_checked(NONCE_LEN)?;
        let plain = self
            .cipher
            .decrypt(XNonce::from_slice(nonce), ciphertext)
            .ok()?;
        postcard::from_bytes(&plain).ok()
    }
}
