//! The host's secrets on disk: its iroh key, the ticket's token and the
//! credential store's key, each 32 bytes as 64 lowercase hex digits.
//!
//! `daemon` reads the file, asks [`stored`] what it holds, and creates the
//! secret when there is none. For the credential store's key it asks
//! [`store_key`] where the key is, from the files the data directory holds.

/// What a secret's file holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Secret {
    Key([u8; 32]),
    /// There is no file: the host creates the secret.
    Missing,
    /// The file is not 64 hex digits.
    Malformed,
}

/// What the file's `text` holds, `None` when there is no file.
#[must_use]
pub fn stored(text: Option<&str>) -> Secret {
    text.map_or(Secret::Missing, |hex| {
        data_encoding::HEXLOWER
            .decode(hex.trim().as_bytes())
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .map_or(Secret::Malformed, Secret::Key)
    })
}

/// What the data directory holds of the credential store and its key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreFiles {
    /// `store.key.slot`: the account of the key's keystore entry.
    pub slot: Option<[u8; 32]>,
    /// `store.key`: the key itself, for a store kept in a file.
    pub file: Option<[u8; 32]>,
    /// Sealed credentials, which only their own key opens.
    pub sealed: bool,
    /// `store.key.file`: a new store keeps its key in a file.
    pub file_only: bool,
}

/// Where the credential store's key is, or where a new one goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreKey {
    /// In the OS keystore, under this slot's account.
    Keystore([u8; 32]),
    /// In `store.key`.
    File([u8; 32]),
    /// A new store, keyed in the OS keystore where it is usable.
    NewInKeystore,
    /// A new store, keyed in `store.key`.
    NewInFile,
    /// Sealed credentials without their key: a new key would never open
    /// them.
    Lost,
}

/// Where the key of the store `found` describes is. A store keeps the
/// place chosen when it was created, and the slot wins over a stray file.
#[must_use]
pub const fn store_key(found: &StoreFiles) -> StoreKey {
    match (found.slot, found.file, found.sealed, found.file_only) {
        (Some(slot), ..) => StoreKey::Keystore(slot),
        (None, Some(key), ..) => StoreKey::File(key),
        (None, None, true, _) => StoreKey::Lost,
        (None, None, false, true) => StoreKey::NewInFile,
        (None, None, false, false) => StoreKey::NewInKeystore,
    }
}

#[cfg(test)]
mod tests {
    use super::{Secret, StoreFiles, StoreKey, store_key, stored};

    #[test]
    fn a_store_key_is_where_its_store_was_created() {
        let slot = Some([1; 32]);
        let file = Some([2; 32]);
        let files = |slot, file, sealed, file_only| StoreFiles {
            slot,
            file,
            sealed,
            file_only,
        };
        assert_eq!(
            [
                store_key(&files(slot, file, true, true)),
                store_key(&files(None, file, true, false)),
                store_key(&files(None, None, true, true)),
                store_key(&files(None, None, false, true)),
                store_key(&files(None, None, false, false)),
            ],
            [
                StoreKey::Keystore([1; 32]),
                StoreKey::File([2; 32]),
                StoreKey::Lost,
                StoreKey::NewInFile,
                StoreKey::NewInKeystore,
            ]
        );
    }

    #[test]
    fn a_secret_is_64_hex_digits_or_missing() {
        let hex = "ab".repeat(32);
        assert_eq!(
            [
                stored(Some(&format!("{hex}\n"))),
                stored(None),
                stored(Some("nope")),
                stored(Some(&"ab".repeat(16))),
            ],
            [
                Secret::Key([0xab; 32]),
                Secret::Missing,
                Secret::Malformed,
                Secret::Malformed,
            ]
        );
    }
}
