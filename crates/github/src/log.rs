use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use home::UnixMillis;
use home::issue::{Entry, Note};

const OPEN: &str = "<!-- conductor-log\n";
const CLOSE: &str = "\n-->\n";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Posted {
    pub id: String,
    pub author: String,
    pub created: UnixMillis,
    pub body: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Log {
    pub entries: Vec<Entry>,
    pub notes: Vec<Note>,
}

#[must_use]
pub fn comment(entry: &Entry, marker: &str) -> String {
    format!("{OPEN}{marker}{CLOSE}{entry}\n")
}

#[must_use]
pub fn read(posted: Vec<Posted>) -> Log {
    let mut log = Log::default();
    for comment in posted {
        let body = comment.body.replace("\r\n", "\n");
        if crate::canvas::read(&body).is_some() {
            continue;
        }
        match body
            .strip_prefix(OPEN)
            .and_then(|marked| marked.split_once(CLOSE))
        {
            Some((_, said)) => log.entries.push(Entry::of(said.trim())),
            None => log.notes.push(Note {
                id: comment.id,
                author: comment.author,
                created: comment.created,
                text: comment.body,
            }),
        }
    }
    log
}

#[cfg(test)]
mod tests;
