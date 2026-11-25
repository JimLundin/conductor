use alloc::collections::BTreeMap;
use alloc::string::ToString;

use super::body::Body;
use crate::issue::{ID, KIND, STAGE, STATUS, TITLE};
use toml::{Spanned, Table, Value};
use toml_parser::{Source, lexer::TokenKind};

use crate::prelude::*;

/// An issue's front matter, read: its own words, each where it is text,
/// and the rest by key.
#[derive(Debug, Clone, PartialEq)]
pub struct Front {
    pub id: Option<String>,
    pub title: Option<String>,
    pub kind: Option<String>,
    pub status: Option<String>,
    pub stage: Option<String>,
    pub rest: Table,
}

/// An issue file, as its parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    front: Vec<String>,
    pub body: Body,
}

impl Document {
    /// The document `text` holds.
    ///
    /// # Errors
    ///
    /// It has no front matter.
    pub fn of(text: &str) -> Result<Self, String> {
        let unix = text
            .strip_prefix('\u{feff}')
            .unwrap_or(text)
            .replace("\r\n", "\n");
        let missing = || String::from("it has no +++ front matter");
        let mut lines = unix.split('\n');
        if lines.next() != Some("+++") {
            return Err(missing());
        }
        let mut front = Vec::new();
        loop {
            match lines.next() {
                Some("+++") => break,
                Some(line) => front.push(String::from(line)),
                None => return Err(missing()),
            }
        }
        let rest = lines.collect::<Vec<&str>>().join("\n");
        Ok(Self {
            front,
            body: Body::of(&rest),
        })
    }

    #[must_use]
    pub fn new(front: &[(String, String)], body: Body) -> Self {
        Self {
            front: front
                .iter()
                .map(|(key, value)| format!("{} = {value}", key_text(key)))
                .collect(),
            body,
        }
    }

    #[must_use]
    pub fn text(&self) -> String {
        format!("+++\n{}\n+++\n{}", self.front.join("\n"), self.body.text())
    }

    fn front(&self) -> String {
        self.front.join("\n")
    }

    /// The front matter, read.
    ///
    /// # Errors
    ///
    /// It isn't TOML.
    pub fn front_matter(&self) -> Result<Front, String> {
        let mut rest = toml::from_str::<Table>(&self.front())
            .map_err(|err| format!("its front matter: {err}"))?;
        let mut word = |key: &str| match rest.remove(key) {
            Some(Value::String(text)) => Some(text),
            _ => None,
        };
        let (id, title, kind, status, stage) =
            (word(ID), word(TITLE), word(KIND), word(STATUS), word(STAGE));
        Ok(Front {
            id,
            title,
            kind,
            status,
            stage,
            rest,
        })
    }

    /// Sets `key` in the front matter to `value`, given as TOML, in its
    /// place, or after the last value when it has none; a list keeps the
    /// comments it had.
    pub fn set(&mut self, key: &str, value: &str) -> Option<()> {
        let mut front = self.front();
        let spans =
            toml::from_str::<BTreeMap<String, Spanned<Value>>>(&front).ok()?;
        if let Some(found) = spans.get(key) {
            let comments = array_with_comments(front.get(found.span())?, value);
            front.replace_range(
                found.span(),
                comments.as_deref().unwrap_or(value),
            );
        } else {
            let last_value = spans
                .values()
                .filter(|item| !item.get_ref().is_table())
                .map(|item| item.span().end)
                .max();
            if let Some(end) = last_value {
                let at = front
                    .get(end..)?
                    .find('\n')
                    .map_or(front.len(), |offset| end + offset);
                front.insert_str(at, &format!("\n{} = {value}", key_text(key)));
            } else {
                front.insert_str(0, &format!("{} = {value}\n", key_text(key)));
            }
        }
        self.front = front.split('\n').map(String::from).collect();
        Some(())
    }

    #[must_use]
    pub fn comments(&self) -> Vec<&str> {
        self.front
            .iter()
            .filter_map(|line| line.strip_prefix("# "))
            .collect()
    }

    /// Ends the front matter with a comment saying `text`.
    pub fn comment(&mut self, text: &str) {
        self.front.push(comment_line(text));
    }
}

/// The front matter's line of a comment saying `text`.
#[must_use]
pub fn comment_line(text: &str) -> String {
    format!("# {text}")
}

fn array_with_comments(previous: &str, value: &str) -> Option<String> {
    let elements = value.strip_prefix('[')?.strip_suffix(']')?;
    let source = Source::new(previous);
    let mut comments = String::new();
    for token in source
        .lex()
        .filter(|token| token.kind() == TokenKind::Comment)
    {
        let comment = source.get(token)?;
        comments.push_str("    ");
        comments.push_str(comment.as_str());
        comments.push('\n');
    }
    if comments.is_empty() {
        return None;
    }
    if !elements.is_empty() {
        comments.push_str("    ");
        comments.push_str(elements);
        comments.push('\n');
    }
    Some(format!("[\n{comments}]"))
}

fn key_text(key: &str) -> String {
    if !key.is_empty()
        && key.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
        })
    {
        String::from(key)
    } else {
        Value::String(String::from(key)).to_string()
    }
}
