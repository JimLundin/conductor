use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use home::canvas::{Canvas, CanvasError, CanvasName, Held, Stray, StrayReason};
use home::issue::{Comment, CommentWrite, IssueComment};
use home::{IssueId, UnixMillis};

pub const LIMIT: usize = 65_536;

const OPEN: &str = "<!-- conductor-canvas\n";
const CLOSE: &str = "\n-->\n";
const DATA: &str = "<details>\n<summary>Canvas data</summary>\n\n```json\n";
const FENCE: &str = "\n```\n";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TooLong {
    pub size: usize,
}

impl fmt::Display for TooLong {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            out,
            "its comment would be {} characters, over GitHub's limit of {LIMIT}: split the canvas",
            self.size
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Read {
    Canvas {
        name: CanvasName,
        by: String,
        canvas: Result<Canvas, CanvasError>,
    },
    Stray(StrayReason),
}

pub fn comment(
    name: &CanvasName,
    by: &str,
    marker: &str,
    canvas: &Canvas,
) -> Result<String, TooLong> {
    let signer = serde_json::to_string(by)
        .unwrap_or_default()
        .replace('-', "\\u002d");
    let body = format!(
        "{OPEN}name: {name}\nby: {signer}\n{marker}{CLOSE}_Conductor keeps this canvas here, and `home canvas put` rewrites this comment on every put, so an edit to it is lost._\n\n{}\n{DATA}{}{FENCE}\n</details>\n",
        canvas.markdown(),
        canvas.encode()
    );
    let size = body.chars().count();
    if size > LIMIT {
        return Err(TooLong { size });
    }
    Ok(body)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub issue: IssueId,
    pub name: CanvasName,
    pub by: String,
    pub updated: UnixMillis,
    pub canvas: Result<Canvas, CanvasError>,
}

#[must_use]
pub fn marked(comments: Vec<IssueComment>) -> Vec<IssueComment> {
    comments
        .into_iter()
        .filter(|kept| kept.comment.body.replace("\r\n", "\n").contains(OPEN))
        .collect()
}

#[must_use]
pub fn write(
    comments: &[Comment],
    name: &CanvasName,
    body: String,
) -> CommentWrite {
    match comments.iter().find(|comment| holds(comment, name)) {
        Some(held) => CommentWrite::Edit {
            comment: held.id.clone(),
            body,
        },
        None => CommentWrite::Create { body },
    }
}

fn holds(comment: &Comment, name: &CanvasName) -> bool {
    matches!(read(&comment.body), Some(Read::Canvas { name: held, .. }) if held == *name)
}

#[must_use]
pub fn found(
    comments: Vec<IssueComment>,
    issue: Option<&IssueId>,
) -> Held<Found> {
    let mut held = Held::default();
    let mut seen = BTreeSet::new();
    for IssueComment { issue: on, comment } in comments
        .into_iter()
        .filter(|kept| issue.is_none_or(|wanted| *wanted == kept.issue))
    {
        let placed = |reason| Stray {
            place: comment.url.clone(),
            reason,
        };
        match read(&comment.body) {
            None => {}
            Some(Read::Stray(reason)) => held.strays.push(placed(reason)),
            Some(Read::Canvas { name, by, canvas }) => {
                if seen.insert((on.clone(), name.clone())) {
                    held.canvases.push(Found {
                        issue: on,
                        name,
                        by,
                        updated: comment.updated,
                        canvas,
                    });
                } else {
                    held.strays.push(placed(StrayReason::Repeated));
                }
            }
        }
    }
    held.canvases.sort_by(|one, other| {
        (&one.issue, &one.name).cmp(&(&other.issue, &other.name))
    });
    held
}

#[must_use]
pub fn read(text: &str) -> Option<Read> {
    let body = text.replace("\r\n", "\n");
    let (_, opened) = body.split_once(OPEN)?;
    let Some((marker, rest)) = opened.split_once(CLOSE) else {
        return Some(Read::Stray(StrayReason::Unreadable));
    };
    let Some(name) =
        field(marker, "name").and_then(|text| CanvasName::parse(text).ok())
    else {
        return Some(Read::Stray(StrayReason::Unnamed));
    };
    let signer = field(marker, "by")
        .and_then(|signed| serde_json::from_str::<String>(signed).ok());
    let kept = rest
        .rsplit_once(DATA)
        .and_then(|(_, data)| data.split_once(FENCE))
        .map(|(data, _)| data);
    let canvas = match (&signer, kept) {
        (None, _) => Err(unreadable("its marker names no signer")),
        (Some(_), None) => Err(unreadable("it holds no canvas data")),
        (Some(_), Some(data)) => Canvas::parse(data),
    };
    Some(Read::Canvas {
        name,
        by: signer.unwrap_or_default(),
        canvas,
    })
}

fn unreadable(reason: &str) -> CanvasError {
    CanvasError::Json {
        reason: String::from(reason),
    }
}

fn field<'a>(marker: &'a str, key: &str) -> Option<&'a str> {
    marker.lines().find_map(|line| {
        line.strip_prefix(key)
            .and_then(|rest| rest.strip_prefix(": "))
    })
}

#[cfg(test)]
mod tests;
