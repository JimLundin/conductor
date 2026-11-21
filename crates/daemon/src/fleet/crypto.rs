use std::path::Path;

use anyhow::{Context, anyhow, ensure};
use argon2::Argon2;
use chacha20poly1305::{
    Key, XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use iroh::{PublicKey, SecretKey, Signature};
use proto::fleet::{Certificate, Configuration, Identity, SignedConfiguration};
use serde::{Serialize, de::DeserializeOwned};
use zeroize::Zeroizing;

use proto::fleet::CERTIFICATE_DOMAIN;
pub(super) use proto::fleet::SNAPSHOT_DOMAIN;
const CONFIGURATION: &[u8] = b"conductor fleet configuration v1";
const STORE: &[u8] = b"conductor fleet local store v1";
const BACKUP: &[u8] = b"conductor fleet root backup v1";

pub(super) fn sign_identity(
    key: &SecretKey,
    identity: Identity,
) -> anyhow::Result<Certificate> {
    let signature = sign(key, CERTIFICATE_DOMAIN, &identity)?;
    Ok(Certificate {
        identity,
        signature,
    })
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
    let public_signature = sign(key, SNAPSHOT_DOMAIN, &config.snapshot)?;
    Ok(SignedConfiguration {
        config,
        signature,
        public_signature,
    })
}

pub(super) fn verify_identity(
    key: PublicKey,
    certificate: &Certificate,
) -> anyhow::Result<()> {
    verify(
        key,
        CERTIFICATE_DOMAIN,
        &certificate.identity,
        &certificate.signature,
    )
}

pub(super) fn verify_config(
    key: PublicKey,
    signed: &SignedConfiguration,
) -> anyhow::Result<()> {
    verify(key, CONFIGURATION, &signed.config, &signed.signature)?;
    verify(
        key,
        SNAPSHOT_DOMAIN,
        &signed.config.snapshot,
        &signed.public_signature,
    )
}

fn sign<T: Serialize>(
    key: &SecretKey,
    domain: &[u8],
    value: &T,
) -> anyhow::Result<Vec<u8>> {
    let bytes = postcard::to_stdvec(&(domain, value))?;
    Ok(key.sign(&bytes).to_bytes().to_vec())
}

pub(super) fn verify<T: Serialize>(
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
