use std::path::Path;

use anyhow::{Context, anyhow, ensure};
use argon2::Argon2;
use chacha20poly1305::{
    Key, XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use host::fleet::{
    Certificate, Configuration, Identity, Signed, SignedConfiguration,
};
use iroh::{PublicKey, SecretKey, Signature};
use serde::{Serialize, de::DeserializeOwned};
use zeroize::Zeroizing;

const CONFIGURATION: &[u8] = b"conductor fleet configuration v3";
const STORE: &[u8] = b"conductor fleet local store v3";
const BACKUP: &[u8] = b"conductor fleet root backup v3";

pub(super) fn sign_identity(
    key: &SecretKey,
    identity: &Identity,
) -> anyhow::Result<Certificate> {
    sign_payload(key, identity)
}

/// `value` as JSON, signed by `key` over its domain then those bytes.
fn sign_payload<T: host::fleet::Payload>(
    key: &SecretKey,
    value: &T,
) -> anyhow::Result<Signed<T>> {
    let payload = serde_json::to_vec(value)?;
    let signature = key.sign(&message::<T>(&payload)).to_bytes().to_vec();
    Ok(Signed::new(payload, signature))
}

/// The value `signed` holds, parsed only once its signature by `key`
/// verifies over the bytes it carries.
pub(super) fn open<T: host::fleet::Payload>(
    key: PublicKey,
    signed: &Signed<T>,
) -> anyhow::Result<T> {
    let signature = Signature::from_bytes(
        signed
            .signature
            .as_slice()
            .try_into()
            .context("invalid signature length")?,
    );
    key.verify(&message::<T>(&signed.payload), &signature)
        .context("the signature does not verify")?;
    Ok(serde_json::from_slice(&signed.payload)?)
}

fn message<T: host::fleet::Payload>(payload: &[u8]) -> Vec<u8> {
    T::DOMAIN.iter().chain(payload).copied().collect()
}

pub(super) fn sign_config(
    key: &SecretKey,
    config: Configuration,
) -> anyhow::Result<SignedConfiguration> {
    ensure!(
        postcard::to_stdvec(&config)?.len() <= 200 * 1024,
        "fleet configuration is too large"
    );
    let signature = sign(key, CONFIGURATION, &config)?;
    let public = sign_payload(key, &config.snapshot)?;
    Ok(SignedConfiguration {
        config,
        signature,
        public,
    })
}

pub(super) fn verify_config(
    key: PublicKey,
    signed: &SignedConfiguration,
) -> anyhow::Result<()> {
    verify(key, CONFIGURATION, &signed.config, &signed.signature)?;
    ensure!(
        open(key, &signed.public)? == signed.config.snapshot,
        "the public snapshot is not the configuration's"
    );
    Ok(())
}

fn sign<T: Serialize>(
    key: &SecretKey,
    domain: &[u8],
    value: &T,
) -> anyhow::Result<Vec<u8>> {
    let bytes = postcard::to_stdvec(&(domain, value))?;
    Ok(key.sign(&bytes).to_bytes().to_vec())
}

fn verify<T: Serialize>(
    key: PublicKey,
    domain: &[u8],
    value: &T,
    signature: &[u8],
) -> anyhow::Result<()> {
    let bytes = postcard::to_stdvec(&(domain, value))?;
    let signed = Signature::from_bytes(
        signature.try_into().context("invalid signature length")?,
    );
    key.verify(&bytes, &signed)?;
    Ok(())
}

pub(super) fn seal<T: Serialize>(
    key: &[u8; 32],
    state: &T,
) -> anyhow::Result<Vec<u8>> {
    let bytes = Zeroizing::new(postcard::to_stdvec(state)?);
    encrypt(key, STORE, &bytes)
}

pub(super) fn unseal<T: DeserializeOwned>(
    key: &[u8; 32],
    file: &[u8],
) -> anyhow::Result<T> {
    let bytes = Zeroizing::new(decrypt(key, STORE, file)?);
    Ok(postcard::from_bytes(&bytes)?)
}

pub(super) fn backup<T: Serialize>(
    path: &Path,
    passphrase: &[u8],
    value: &T,
) -> anyhow::Result<()> {
    ensure!(
        !passphrase.is_empty(),
        "the fleet backup needs a passphrase"
    );
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt)?;
    let key = backup_key(passphrase, &salt)?;
    let encoded = Zeroizing::new(postcard::to_stdvec(value)?);
    let sealed = encrypt(&key, BACKUP, &encoded)?;
    let file = salt.into_iter().chain(sealed).collect::<Vec<_>>();
    create_file(path, &file)
}

pub(super) fn recover<T: DeserializeOwned>(
    path: &Path,
    passphrase: &[u8],
) -> anyhow::Result<T> {
    use std::io::Read;

    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(256 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 256 * 1024, "invalid fleet backup length");
    let (salt, sealed) = bytes
        .split_at_checked(16)
        .context("invalid fleet backup length")?;
    let key = backup_key(passphrase, salt)?;
    let decoded = Zeroizing::new(decrypt(&key, BACKUP, sealed)?);
    Ok(postcard::from_bytes(&decoded)?)
}

fn backup_key(
    passphrase: &[u8],
    salt: &[u8],
) -> anyhow::Result<Zeroizing<[u8; 32]>> {
    let mut key = Zeroizing::new([0u8; 32]);
    Argon2::default()
        .hash_password_into(passphrase, salt, key.as_mut())
        .map_err(|_| anyhow!("could not derive the fleet backup key"))?;
    Ok(key)
}

fn encrypt(
    key: &[u8; 32],
    aad: &[u8],
    bytes: &[u8],
) -> anyhow::Result<Vec<u8>> {
    let mut nonce = [0u8; 24];
    getrandom::fill(&mut nonce)?;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    let ciphertext = cipher
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: bytes, aad })
        .map_err(|_| anyhow!("could not seal fleet data"))?;
    Ok(nonce.into_iter().chain(ciphertext).collect())
}

fn decrypt(key: &[u8; 32], aad: &[u8], file: &[u8]) -> anyhow::Result<Vec<u8>> {
    let (nonce, ciphertext) = file
        .split_at_checked(24)
        .context("invalid fleet file length")?;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| anyhow!("could not open sealed fleet data"))
}

pub(super) fn create_file(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let dir = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    crate::with_private_file(dir, bytes, |next| {
        std::fs::hard_link(next, path)
    })?;
    crate::sync_dir(dir)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use anyhow::Context;
    use host::fleet::{HostEntry, Identity, Role, Snapshot};
    use iroh::SecretKey;

    use super::{open, sign_identity, sign_payload};

    fn key() -> SecretKey {
        SecretKey::from_bytes(&[3; 32])
    }

    /// The same payload with one byte changed, here a letter of the name.
    fn changed(payload: &[u8], from: &[u8], to: u8) -> anyhow::Result<Vec<u8>> {
        let at = payload
            .windows(from.len())
            .position(|bytes| bytes == from)
            .context("the payload holds the text")?;
        let mut changed = payload.to_vec();
        *changed.get_mut(at).context("in range")? = to;
        Ok(changed)
    }

    #[test]
    fn a_browser_certificate_verifies_over_the_json_it_was_signed_as()
    -> anyhow::Result<()> {
        let identity = Identity {
            fleet: [1; 32],
            device: [2; 32],
            name: String::from("phone"),
            role: Role::Owner,
        };
        let certificate = sign_identity(&key(), &identity)?;
        let ones = ["1"; 32].join(",");
        let twos = ["2"; 32].join(",");
        assert_eq!(
            String::from_utf8(certificate.payload.clone())?,
            format!(
                r#"{{"fleet":[{ones}],"device":[{twos}],"name":"phone","role":"Owner"}}"#
            )
        );
        assert_eq!(open(key().public(), &certificate)?, identity);

        let mut renamed = certificate.clone();
        renamed.payload = changed(&certificate.payload, b"phone", b'P')?;
        assert_eq!(
            open(key().public(), &renamed).map_err(|error| error.to_string()),
            Err(String::from("the signature does not verify"))
        );
        let mut spaced = certificate.clone();
        spaced.payload =
            b" ".iter().chain(&certificate.payload).copied().collect();
        assert!(open(key().public(), &spaced).is_err());
        let other = SecretKey::from_bytes(&[4; 32]).public();
        assert!(open(other, &certificate).is_err());
        Ok(())
    }

    #[test]
    fn a_fleet_snapshot_verifies_over_the_json_it_was_signed_as()
    -> anyhow::Result<()> {
        let snapshot = Snapshot {
            fleet: [1; 32],
            root: [2; 32],
            version: 7,
            hosts: Vec::from([HostEntry {
                node: [2; 32],
                name: String::from("desk"),
                relay: String::from("https://relay.example/"),
                added_at: 9,
            }]),
            revoked: Vec::new(),
        };
        let signed = sign_payload(&key(), &snapshot)?;
        assert_eq!(open(key().public(), &signed)?, snapshot);
        let mut renamed = signed.clone();
        renamed.payload = changed(&signed.payload, b"desk", b'D')?;
        assert_eq!(
            open(key().public(), &renamed).map_err(|error| error.to_string()),
            Err(String::from("the signature does not verify"))
        );
        let certificate = sign_identity(
            &key(),
            &Identity {
                fleet: [1; 32],
                device: [2; 32],
                name: String::from("desk"),
                role: Role::Host,
            },
        )?;
        let mut posing = signed;
        posing.signature = certificate.signature;
        assert!(open(key().public(), &posing).is_err());
        Ok(())
    }

    #[test]
    fn previous_fleet_store_context_is_rejected() -> anyhow::Result<()> {
        let key = [17; 32];
        let old =
            super::encrypt(&key, b"conductor fleet local store v2", &[7])?;
        assert!(super::unseal::<u8>(&key, &old).is_err());
        assert_eq!(super::unseal::<u8>(&key, &super::seal(&key, &7u8)?)?, 7);
        Ok(())
    }

    #[test]
    fn previous_fleet_backup_context_is_rejected() -> anyhow::Result<()> {
        let binary = std::env::current_exe()?;
        let directory = tempfile::tempdir_in(
            binary.parent().context("test binary parent")?,
        )?;
        let path = directory.path().join("backup");
        let phrase = b"synthetic test passphrase";
        let salt = [18; 16];
        let key = super::backup_key(phrase, &salt)?;
        let old =
            super::encrypt(&key, b"conductor fleet root backup v2", &[7])?;
        std::fs::write(&path, salt.into_iter().chain(old).collect::<Vec<_>>())?;
        assert!(super::recover::<u8>(&path, phrase).is_err());
        let current = directory.path().join("current-backup");
        super::backup(&current, phrase, &7u8)?;
        assert_eq!(super::recover::<u8>(&current, phrase)?, 7);
        Ok(())
    }
}
