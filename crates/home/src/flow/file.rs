//! An issue file's layout (docs/home.md §3), known nowhere else: `+++`
//! TOML front matter, then the body's sections, the `## Checklist` and the
//! `## Notes` log, each heading read only outside code fences.
//!
//! A file is read once into a [`Document`]. The graph takes an issue's
//! parts from its reading, and its edits change the lines that reading
//! found, so an edit changes what was read and only that: comments,
//! sections of a person's own and the order of it all stay as written. A
//! byte-order mark and Windows line ends read as if absent, and a document
//! is written without them.

use alloc::collections::BTreeMap;
use alloc::string::ToString;

use proto::{ChecklistItem, Entry, Section};
use toml::{Spanned, Table, Value};
use toml_parser::{Source, lexer::TokenKind};

use crate::prelude::*;

/// The keys of an issue's own words in its front matter, beside the
/// fields and relations the flow declares.
pub const ID: &str = "id";
pub const TITLE: &str = "title";
pub const KIND: &str = "kind";
pub const STATUS: &str = "status";
pub const STAGE: &str = "stage";

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
    /// The front matter's lines, between the `+++` lines.
    front: Vec<String>,
    /// The lines after the closing `+++`.
    rest: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Section,
    Checklist,
    Notes,
}

impl Part {
    const fn heading(self) -> &'static str {
        match self {
            Self::Section => "",
            Self::Checklist => "Checklist",
            Self::Notes => "Notes",
        }
    }
}

/// What a line after the front matter is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Place {
    part: Part,
    /// It is the part's `## ` heading.
    heading: bool,
    /// It is outside code fences.
    outside: bool,
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
            rest: rest.lines().map(String::from).collect(),
        })
    }

    /// A new issue's document: its front matter's keys with their values
    /// as TOML, a heading for each of `sections`, and a log of `first`.
    #[must_use]
    pub fn new(
        front: &[(String, String)],
        sections: &[String],
        first: &Entry,
    ) -> Self {
        let mut rest = Vec::from([String::new()]);
        for heading in sections {
            rest.extend([format!("## {heading}"), String::new()]);
        }
        rest.extend([format!("## {}", Part::Notes.heading()), String::new()]);
        rest.extend(log_lines(first));
        Self {
            front: front
                .iter()
                .map(|(key, value)| format!("{key} = {value}"))
                .collect(),
            rest,
        }
    }

    #[must_use]
    pub fn text(&self) -> String {
        let rest = self
            .rest
            .iter()
            .fold(String::new(), |text, line| format!("{text}{line}\n"));
        format!("+++\n{}\n+++\n{rest}", self.front.join("\n"))
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

    /// The body's sections in order, the text before the first heading as
    /// one with no heading; the checklist and the log aren't among them.
    #[must_use]
    pub fn sections(&self) -> Vec<Section> {
        let mut sections: Vec<Section> = Vec::new();
        for (line, place) in self.placed() {
            if place.part != Part::Section {
                continue;
            }
            if place.heading {
                sections.push(Section {
                    heading: heading_of(line).to_owned(),
                    text: String::new(),
                });
                continue;
            }
            if sections.is_empty() {
                sections.push(Section {
                    heading: String::new(),
                    text: String::new(),
                });
            }
            if let Some(section) = sections.last_mut() {
                section.text.push_str(line);
                section.text.push('\n');
            }
        }
        for section in &mut sections {
            section.text = section.text.trim().to_owned();
        }
        sections.retain(|section| {
            !section.heading.is_empty() || !section.text.is_empty()
        });
        sections
    }

    /// The body before the checklist and the log, as its lines have it.
    #[must_use]
    pub fn body_text(&self) -> String {
        self.rest
            .get(..self.body_end())
            .map_or_else(String::new, |lines| {
                lines.join(
                    "
",
                )
            })
            .trim()
            .to_owned()
    }

    /// The checklist's items.
    #[must_use]
    pub fn checklist(&self) -> Vec<ChecklistItem> {
        self.items().map(|(_, found)| found).collect()
    }

    /// The log, oldest first: each line, and the lines indented under it
    /// by two spaces as more of its text.
    #[must_use]
    pub fn log(&self) -> Vec<Entry> {
        let mut log: Vec<Entry> = Vec::new();
        for (line, place) in self.placed() {
            if place.part != Part::Notes || place.heading || !place.outside {
                continue;
            }
            if let Some(entry) = line.strip_prefix("- ") {
                log.push(Entry::of(entry));
            } else if let (Some(more), Some(last)) =
                (line.strip_prefix("  "), log.last_mut())
            {
                last.text.push('\n');
                last.text.push_str(more);
            }
        }
        log
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
                front.insert_str(at, &format!("\n{key} = {value}"));
            } else {
                front.insert_str(0, &format!("{key} = {value}\n"));
            }
        }
        self.front = front.split('\n').map(String::from).collect();
        Some(())
    }

    /// The front matter has a comment saying `text`.
    #[must_use]
    pub fn has_comment(&self, text: &str) -> bool {
        self.front.contains(&comment_line(text))
    }

    /// Ends the front matter with a comment saying `text`.
    pub fn comment(&mut self, text: &str) {
        self.front.push(comment_line(text));
    }

    /// Replaces the body before the checklist and the log with `body`.
    pub fn body(&mut self, body: &str) {
        let end = self.body_end();
        let mut lines = Vec::from([String::new()]);
        lines.extend(body.lines().map(String::from));
        if end < self.rest.len() {
            lines.push(String::new());
        }
        self.rest.splice(..end, lines);
    }

    pub fn add_item(&mut self, text: &str) {
        let added = ChecklistItem {
            done: false,
            text: String::from(text),
        };
        self.append(
            Part::Checklist,
            Some(Part::Notes),
            Vec::from([checklist_line(&added)]),
        );
    }

    /// Checks or unchecks the checklist's item at `index`.
    pub fn check(&mut self, index: usize, done: bool) -> Option<()> {
        let (at, found) = self.items().nth(index)?;
        *self.rest.get_mut(at)? = checklist_line(&ChecklistItem {
            done,
            text: found.text,
        });
        Some(())
    }

    /// Removes the checklist's item at `index`.
    pub fn remove_item(&mut self, index: usize) -> Option<()> {
        let (at, _) = self.items().nth(index)?;
        self.rest.remove(at);
        Some(())
    }

    /// Adds `entry` to the end of the log.
    pub fn log_entry(&mut self, entry: &Entry) {
        self.append(Part::Notes, None, log_lines(entry));
    }

    /// Each line after the front matter, with what it is.
    fn placed(&self) -> impl Iterator<Item = (&str, Place)> {
        let mut part = Part::Section;
        markdown(&self.rest).map(move |(line, outside)| {
            let heading = line.strip_prefix("## ").filter(|_| outside);
            if let Some(named) = heading {
                part = match named.trim() {
                    "Checklist" => Part::Checklist,
                    "Notes" => Part::Notes,
                    _ => Part::Section,
                };
            }
            (
                line,
                Place {
                    part,
                    heading: heading.is_some(),
                    outside,
                },
            )
        })
    }

    fn items(&self) -> impl Iterator<Item = (usize, ChecklistItem)> + '_ {
        self.placed().enumerate().filter_map(|(at, (line, place))| {
            (place.part == Part::Checklist && !place.heading && place.outside)
                .then(|| item(line))
                .flatten()
                .map(|found| (at, found))
        })
    }

    /// Where the body ends: at the checklist or the log, whichever is
    /// first.
    fn body_end(&self) -> usize {
        self.placed()
            .position(|(_, place)| place.heading && place.part != Part::Section)
            .unwrap_or(self.rest.len())
    }

    /// The first section of `part`, from its heading's line to the next
    /// heading or the end, if the file has one.
    fn section(&self, part: Part) -> Option<(usize, usize)> {
        let mut headings = self
            .placed()
            .enumerate()
            .filter(|(_, (_, place))| place.heading);
        let (start, _) = headings.find(|(_, (_, place))| place.part == part)?;
        let end = headings.next().map_or(self.rest.len(), |(index, _)| index);
        Some((start, end))
    }

    /// Adds `lines` after the last line with text of the section of
    /// `part`, or as a new section: before the one of `before`, or at the
    /// end.
    fn append(&mut self, part: Part, before: Option<Part>, lines: Vec<String>) {
        if let Some((start, end)) = self.section(part) {
            let last = (start..end)
                .rev()
                .find(|at| {
                    self.rest
                        .get(*at)
                        .is_some_and(|line| !line.trim().is_empty())
                })
                .unwrap_or(start);
            let after = last + 1;
            self.rest.splice(after..after, lines);
            return;
        }
        let mut section = Vec::from([format!("## {}", part.heading())]);
        section.extend(lines);
        if let Some((at, _)) = before.and_then(|other| self.section(other)) {
            section.push(String::new());
            self.rest.splice(at..at, section);
        } else {
            if self.rest.last().is_some_and(|line| !line.trim().is_empty()) {
                self.rest.push(String::new());
            }
            self.rest.extend(section);
        }
    }
}

/// The front matter's line of a comment saying `text`.
#[must_use]
pub fn comment_line(text: &str) -> String {
    format!("# {text}")
}

/// A checklist item as its line has it.
#[must_use]
pub fn checklist_line(item: &ChecklistItem) -> String {
    let mark = if item.done { 'x' } else { ' ' };
    format!("- [{mark}] {}", item.text)
}

/// An entry as the log has it: `-` and its text, with each further line of
/// the text indented under it by two spaces.
#[must_use]
pub fn log_line(entry: &Entry) -> String {
    log_lines(entry).join("\n")
}

fn log_lines(entry: &Entry) -> Vec<String> {
    entry
        .to_string()
        .split('\n')
        .enumerate()
        .map(|(at, line)| {
            if at == 0 {
                format!("- {line}")
            } else {
                format!("  {line}")
            }
        })
        .collect()
}

fn item(line: &str) -> Option<ChecklistItem> {
    let (done, text) = line
        .strip_prefix("- [x] ")
        .or_else(|| line.strip_prefix("- [X] "))
        .map(|text| (true, text))
        .or_else(|| line.strip_prefix("- [ ] ").map(|text| (false, text)))?;
    Some(ChecklistItem {
        done,
        text: String::from(text),
    })
}

fn heading_of(line: &str) -> &str {
    line.strip_prefix("## ").unwrap_or(line).trim()
}

/// Each line, and whether it is outside code fences.
fn markdown(rest: &[String]) -> impl Iterator<Item = (&str, bool)> {
    let mut fence = None;
    rest.iter().map(move |line| {
        let previously_open = fence.is_some();
        if let Some((marker, width, suffix)) = fence_line(line) {
            match fence {
                Some((open, minimum))
                    if open == marker
                        && width >= minimum
                        && suffix.trim().is_empty() =>
                {
                    fence = None;
                }
                None if marker == b'~' || !suffix.contains('`') => {
                    fence = Some((marker, width));
                }
                _ => {}
            }
        }
        (line.as_str(), !previously_open && fence.is_none())
    })
}

fn fence_line(line: &str) -> Option<(u8, usize, &str)> {
    let rest = line.trim_start_matches(' ');
    if line.len() - rest.len() > 3 {
        return None;
    }
    let marker = *rest.as_bytes().first()?;
    let width = rest.bytes().take_while(|byte| *byte == marker).count();
    if width < 3 || !matches!(marker, b'`' | b'~') {
        return None;
    }
    Some((marker, width, rest.get(width..)?))
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
