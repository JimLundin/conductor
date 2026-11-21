use alloc::collections::BTreeSet;

use proto::transcript::{MAX_TRANSCRIPT_FILES, Transcript, TranscriptFile};
use serde_json::Value;
use time::PrimitiveDateTime;
use time::macros::format_description;
use uuid::Uuid;

use crate::prelude::*;

pub const SESSIONS: &str = "sessions";
pub const ARCHIVED: &str = "archived_sessions";

pub struct Rollout {
    pub thread: Uuid,
    id: Uuid,
    active: bool,
}

fn identity(text: &str) -> Option<Uuid> {
    Uuid::parse_str(text)
        .ok()
        .filter(|id| id.to_string() == text)
}

/// Parses the trusted native thread IDs named by Conductor history.
///
/// # Errors
/// A thread ID is missing or is not in the CLI UUID format.
pub fn threads(ids: &[String]) -> Result<BTreeSet<Uuid>, &'static str> {
    if ids.is_empty() || ids.len() > MAX_TRANSCRIPT_FILES {
        return Err("invalid Codex conversation count");
    }
    ids.iter()
        .map(|id| identity(id).ok_or("invalid Codex thread ID"))
        .collect()
}

fn filename(name: &str) -> Option<(&str, Uuid, Uuid)> {
    let body = name.strip_prefix("rollout-")?.strip_suffix(".jsonl")?;
    let (timestamp, rest) = body.split_at_checked(19)?;
    PrimitiveDateTime::parse(
        timestamp,
        format_description!("[year]-[month]-[day]T[hour]-[minute]-[second]"),
    )
    .ok()?;
    let ids = rest.strip_prefix('-')?;
    let (thread, rollout) = ids.split_once('_').unwrap_or((ids, ids));
    Some((timestamp, identity(thread)?, identity(rollout)?))
}

#[must_use]
pub fn calendar_component(component: &str, depth: usize) -> bool {
    let width = if depth == 0 { 4 } else { 2 };
    component.len() == width
        && component.bytes().all(|byte| byte.is_ascii_digit())
}

#[must_use]
pub fn rollout(path: &str) -> Option<Rollout> {
    let segments = path.split('/').collect::<Vec<_>>();
    match segments.as_slice() {
        [SESSIONS, year, month, day, name] => {
            let (date, thread, rollout) = filename(name)?;
            (date.get(..4)? == *year
                && date.get(5..7)? == *month
                && date.get(8..10)? == *day)
                .then_some(Rollout {
                    thread,
                    id: rollout,
                    active: true,
                })
        }
        [ARCHIVED, name] => {
            let (_, thread, rollout) = filename(name)?;
            Some(Rollout {
                thread,
                id: rollout,
                active: false,
            })
        }
        _ => None,
    }
}

pub struct Base {
    rollout: Uuid,
    cutoff: usize,
}

/// Reads the rollout identity and its optional retained prefix.
///
/// # Errors
/// The metadata is missing or does not identify the requested thread.
pub fn metadata(
    bytes: &[u8],
    expected: &Rollout,
) -> Result<Option<Base>, &'static str> {
    const INVALID: &str = "invalid or mismatched Codex rollout metadata";
    let line = bytes.split(|byte| *byte == b'\n').next().ok_or(INVALID)?;
    let head = serde_json::from_slice::<Value>(line).map_err(|_| INVALID)?;
    if head.get("type").and_then(Value::as_str) != Some("session_meta")
        || head
            .pointer("/payload/id")
            .and_then(Value::as_str)
            .and_then(identity)
            != Some(expected.thread)
    {
        return Err(INVALID);
    }
    match head.pointer("/payload/history_base") {
        Some(base) if !base.is_null() => {
            let rollout = base
                .get("thread_id")
                .and_then(Value::as_str)
                .and_then(identity)
                .ok_or(INVALID)?;
            let cutoff = base
                .get("end_byte_offset")
                .and_then(Value::as_u64)
                .and_then(|offset| usize::try_from(offset).ok())
                .ok_or(INVALID)?;
            Ok(Some(Base { rollout, cutoff }))
        }
        _ => Ok(None),
    }
}

/// Checks archived and active files for the requested threads.
///
/// # Errors
/// A file, requested thread or retained prefix is invalid or absent.
pub fn validate(
    archive: &Transcript,
    expected: &BTreeSet<Uuid>,
) -> Result<(), &'static str> {
    archive.validate()?;
    let mut active = BTreeSet::new();
    let mut files = BTreeMap::new();
    for file in &archive.files {
        let wire = rollout(&file.path)
            .filter(|wire| expected.contains(&wire.thread))
            .ok_or("Codex archive contains an unrelated path")?;
        let base = metadata(&file.bytes, &wire)?;
        if files.insert(wire.id, (file, base)).is_some() {
            return Err("duplicate Codex rollout identity");
        }
        if wire.active {
            active.insert(wire.thread);
        }
    }
    if active != *expected {
        return Err("active Codex rollout is missing");
    }
    for &(file, ref base) in files.values() {
        validate_history(file, base.as_ref(), &files)?;
    }
    Ok(())
}

fn validate_history<'a>(
    file: &'a TranscriptFile,
    mut base: Option<&'a Base>,
    files: &'a BTreeMap<Uuid, (&'a TranscriptFile, Option<Base>)>,
) -> Result<(), &'static str> {
    let mut ancestry = BTreeSet::new();
    let mut last_file = file;
    while let Some(position) = base {
        let (ancestor, parent) = files
            .get(&position.rollout)
            .ok_or("Codex history prefix is outside the transfer")?;
        if ancestor.path == last_file.path
            || !ancestry.insert(position.rollout)
            || (position.cutoff > 0
                && ancestor.bytes.get(position.cutoff - 1) != Some(&b'\n'))
        {
            return Err("invalid Codex history prefix");
        }
        last_file = ancestor;
        base = parent.as_ref();
    }
    Ok(())
}
