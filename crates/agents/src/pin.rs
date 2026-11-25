use crate::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Package {
    Binary,
    Archive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub package: Package,
    pub binary: String,
}

#[must_use]
pub fn kept(name: &str, keep: &str) -> bool {
    name == keep || name == "lock"
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    pub found: String,
    pub expected: String,
}

pub fn verify(found: String, expected: &str) -> Result<(), Mismatch> {
    if found == expected {
        Ok(())
    } else {
        Err(Mismatch {
            found,
            expected: String::from(expected),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Mismatch, kept, verify};

    #[test]
    fn an_install_keeps_only_the_lock_and_its_version() {
        assert_eq!(
            ["1.0.0", "lock", "0.9.0", "1.0.0.download", "1.0.0.unpack"]
                .map(|name| kept(name, "1.0.0")),
            [true, true, false, false, false]
        );
    }

    #[test]
    fn only_the_pinned_digest_verifies() {
        assert_eq!(
            [
                verify(String::from("ab"), "ab"),
                verify(String::from("cd"), "ab")
            ],
            [
                Ok(()),
                Err(Mismatch {
                    found: String::from("cd"),
                    expected: String::from("ab"),
                })
            ]
        );
    }
}
