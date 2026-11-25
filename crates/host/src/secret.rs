#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Secret {
    Key([u8; 32]),
    Missing,
    Malformed,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreFiles {
    pub slot: Option<[u8; 32]>,
    pub file: Option<[u8; 32]>,
    pub sealed: bool,
    pub file_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreKey {
    Keystore([u8; 32]),
    File([u8; 32]),
    NewInKeystore,
    NewInFile,
    Lost,
}

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
