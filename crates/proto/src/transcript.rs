use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use serde::{Deserialize, Serialize};

pub const MAX_TRANSCRIPT_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_TRANSCRIPT_FILES: usize = 1024;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transcript {
    pub files: Vec<TranscriptFile>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptFile {
    pub path: String,
    pub bytes: Vec<u8>,
}

impl fmt::Debug for Transcript {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("Transcript")
            .field("files", &self.files.len())
            .finish_non_exhaustive()
    }
}

impl Transcript {
    /// Checks the common limits and filename rules before writing any files.
    ///
    /// # Errors
    /// The archive is too large or a file name is unsafe or repeated.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.files.is_empty() || self.files.len() > MAX_TRANSCRIPT_FILES {
            return Err("invalid number of transcript files");
        }
        let mut paths = BTreeSet::new();
        let mut remaining = MAX_TRANSCRIPT_BYTES;
        for file in &self.files {
            if !portable_path(&file.path) {
                return Err("invalid transcript path");
            }
            if !paths.insert(file.path.to_ascii_lowercase()) {
                return Err("duplicate transcript path");
            }
            remaining = remaining
                .checked_sub(file.bytes.len())
                .ok_or("transcript exceeds byte limit")?;
        }
        Ok(())
    }
}

#[must_use]
pub fn portable_path(name: &str) -> bool {
    name.len() <= 1024 && name.split('/').all(portable_component)
}

#[must_use]
pub fn portable_component(name: &str) -> bool {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.ends_with([' ', '.'])
    {
        return false;
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_graphic() || byte == b' ')
        || name.contains(['\\', ':', '*', '?', '"', '<', '>', '|'])
    {
        return false;
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_ascii_uppercase();
    !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|number| {
                number.len() == 1
                    && number.bytes().all(|byte| byte.is_ascii_digit())
            })
}
