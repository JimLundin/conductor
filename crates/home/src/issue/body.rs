use crate::issue::{ChecklistItem, Entry, Section};
use crate::prelude::*;
use alloc::string::ToString;

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(from = "String", into = "String")]
pub struct Body {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Place {
    part: Part,
    is_section_heading: bool,
    outside_code_fence: bool,
}

impl Body {
    #[must_use]
    pub fn new(sections: &[String], first: &Entry) -> Self {
        let mut rest = Vec::from([String::new()]);
        for heading in sections {
            rest.extend([format!("## {heading}"), String::new()]);
        }
        rest.extend([String::from("## Notes"), String::new()]);
        rest.extend(log_lines(first));
        Self { rest }
    }

    /// The document `text` holds.
    ///
    /// # Errors
    ///
    /// It has no front matter.
    #[must_use]
    pub fn of(text: &str) -> Self {
        Self {
            rest: text.lines().map(String::from).collect(),
        }
    }

    #[must_use]
    pub fn text(&self) -> String {
        self.rest
            .iter()
            .fold(String::new(), |text, line| format!("{text}{line}\n"))
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
            if place.is_section_heading {
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
            if place.part != Part::Notes
                || place.is_section_heading
                || !place.outside_code_fence
            {
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

    fn placed(&self) -> impl Iterator<Item = (&str, Place)> {
        let mut part = Part::Section;
        markdown(&self.rest).map(move |(line, outside_code_fence)| {
            let heading =
                line.strip_prefix("## ").filter(|_| outside_code_fence);
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
                    is_section_heading: heading.is_some(),
                    outside_code_fence,
                },
            )
        })
    }

    fn items(&self) -> impl Iterator<Item = (usize, ChecklistItem)> + '_ {
        self.placed().enumerate().filter_map(|(at, (line, place))| {
            (place.part == Part::Checklist
                && !place.is_section_heading
                && place.outside_code_fence)
                .then(|| item(line))
                .flatten()
                .map(|found| (at, found))
        })
    }

    fn body_end(&self) -> usize {
        self.placed()
            .position(|(_, place)| {
                place.is_section_heading && place.part != Part::Section
            })
            .unwrap_or(self.rest.len())
    }

    fn section(&self, part: Part) -> Option<(usize, usize)> {
        let mut headings = self
            .placed()
            .enumerate()
            .filter(|(_, (_, place))| place.is_section_heading);
        let (start, _) = headings.find(|(_, (_, place))| place.part == part)?;
        let end = headings.next().map_or(self.rest.len(), |(index, _)| index);
        Some((start, end))
    }

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

#[derive(Default)]
struct Fences {
    open: Option<(u8, usize)>,
}

enum Line<'a> {
    Prose,
    Opening(&'a str),
    Fenced,
    Closing,
}

impl Fences {
    fn line<'a>(&mut self, line: &'a str) -> Line<'a> {
        if let Some((marker, width, suffix)) = fence_line(line) {
            match self.open {
                Some((open, minimum))
                    if open == marker
                        && width >= minimum
                        && suffix.trim().is_empty() =>
                {
                    self.open = None;
                    return Line::Closing;
                }
                None if marker == b'~' || !suffix.contains('`') => {
                    self.open = Some((marker, width));
                    return Line::Opening(suffix.trim());
                }
                _ => {}
            }
        }
        if self.open.is_some() {
            Line::Fenced
        } else {
            Line::Prose
        }
    }
}

fn markdown(rest: &[String]) -> impl Iterator<Item = (&str, bool)> {
    let mut fences = Fences::default();
    rest.iter().map(move |line| {
        (line.as_str(), matches!(fences.line(line), Line::Prose))
    })
}

/// Splits a final named Markdown fence into its preceding text and contents.
///
/// # Errors
/// A matching block is duplicated, unclosed or followed by nonblank text.
pub fn footer<'a>(
    text: &'a str,
    info: &str,
) -> Result<Option<(&'a str, &'a str)>, String> {
    let mut fences = Fences::default();
    let mut opened = None;
    let mut closed = None;
    let mut offset = 0;
    for raw in text.split_inclusive('\n') {
        let end = offset + raw.len();
        match fences.line(raw.trim_end_matches(['\r', '\n'])) {
            Line::Opening(name) if name == info => {
                if closed.is_some() {
                    return Err(format!("duplicate {info} footer"));
                }
                opened = Some((offset, end));
            }
            Line::Closing => {
                if let Some((start, contents)) = opened.take() {
                    closed = Some((start, contents..offset, end));
                }
            }
            _ => {}
        }
        offset = end;
    }
    if opened.is_some() {
        return Err(format!("unclosed {info} footer"));
    }
    closed
        .map(|(start, contents, end)| {
            if !text[end..].trim().is_empty() {
                return Err(format!(
                    "the {info} footer must be the final block"
                ));
            }
            Ok((&text[..start], &text[contents]))
        })
        .transpose()
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

impl From<String> for Body {
    fn from(text: String) -> Self {
        Self::of(&text)
    }
}

impl From<Body> for String {
    fn from(body: Body) -> Self {
        body.text()
    }
}
