//! The host's secrets on disk: its iroh key, the ticket's token and the
//! credential store's key, each 32 bytes as 64 lowercase hex digits.
//!
//! `daemon` reads the file, asks [`stored`] what it holds, and creates the
//! secret when there is none.

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

#[cfg(test)]
mod tests {
    use super::{Secret, stored};

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
