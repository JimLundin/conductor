use proto::transcript::Transcript;
use serde_json::Value;

use crate::prelude::*;

fn native_uuid(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|parsed| format!("{parsed}") == id)
}

#[must_use]
pub fn allowed(name: &str, is_file: bool) -> bool {
    let mut names = name.split('/');
    let Some(first) = names.next() else {
        return false;
    };
    match (first, names.next(), is_file) {
        ("memory", None, false) | ("memory", Some(_), _) => true,
        (top, None, true) => {
            top.strip_suffix(".jsonl").is_some_and(native_uuid)
        }
        (top, _, _) => native_uuid(top),
    }
}

/// Verifies that the isolated project includes every required conversation.
///
/// # Errors
/// The contents exceed limits, contain invalid files or miss a conversation.
pub fn validate(
    archive: &Transcript,
    ids: &[String],
) -> Result<(), &'static str> {
    archive.validate()?;
    validate_ids(ids)?;
    for file in &archive.files {
        if !allowed(&file.path, true) {
            return Err("unrecognized Claude project file");
        }
        if !file.path.contains('/') {
            let mut lines = file
                .bytes
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.is_empty())
                .peekable();
            if lines.peek().is_none()
                || !lines.all(|line| {
                    serde_json::from_slice::<Value>(line)
                        .is_ok_and(|value| value.is_object())
                })
            {
                return Err("invalid Claude transcript");
            }
        }
    }
    for id in ids {
        let expected = format!("{id}.jsonl");
        if !archive.files.iter().any(|file| file.path == expected) {
            return Err("Claude transcript is missing");
        }
    }
    Ok(())
}

/// # Errors
/// One of the required native IDs is invalid or none were supplied.
pub fn validate_ids(ids: &[String]) -> Result<(), &'static str> {
    if ids.is_empty() || !ids.iter().all(|id| native_uuid(id)) {
        return Err("invalid Claude conversation identity");
    }
    Ok(())
}
