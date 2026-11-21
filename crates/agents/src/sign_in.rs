//! How a CLI signs its private home in when the app asks, as data the
//! provider's core describes: the run, where its output shows the link to
//! open and any code to enter there, whether the page gives a code to paste
//! back on the CLI's stdin, and what its failures mean. The host reads the
//! output line by line against the description; it never keeps or logs it.

use crate::Invocation;
use crate::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignIn {
    /// The run, with its stdin, stdout and stderr on pipes. It succeeds by
    /// exiting 0, after which the profile's sign-in check must pass.
    pub run: Invocation,
    pub link: Pick,
    /// A code the person enters on the page the link opens.
    pub code: Option<Pick>,
    /// The page gives the person a code that the CLI reads as one line on
    /// its stdin.
    pub paste: bool,
    pub failures: Vec<Failure>,
}

/// The first word of its shape after the text `after` in the output, on
/// the same line or a later one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    pub after: String,
    pub word: Word,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Word {
    /// An `https://` URL.
    Link,
    /// At least six capital letters, digits and dashes, as device codes
    /// are written.
    Code,
}

/// A run that printed `says` and then failed failed for the reason
/// `means`, in words for the person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub says: String,
    pub means: String,
}

/// What the person is shown: the link, and the code if the description has
/// one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    pub link: String,
    pub code: Option<String>,
}

impl SignIn {
    /// What the output so far shows, once it shows everything described.
    #[must_use]
    pub fn shown(&self, lines: &[String]) -> Option<Shown> {
        let link = self.link.find(lines)?;
        let code = match &self.code {
            Some(pick) => Some(pick.find(lines)?),
            None => None,
        };
        Some(Shown { link, code })
    }

    /// What it means that the run printed `line`, if a failure says so.
    #[must_use]
    pub fn failure(&self, line: &str) -> Option<&str> {
        self.failures
            .iter()
            .find(|failure| line.contains(failure.says.as_str()))
            .map(|failure| failure.means.as_str())
    }
}

impl Pick {
    #[must_use]
    pub fn find(&self, lines: &[String]) -> Option<String> {
        let start = lines
            .iter()
            .position(|line| line.contains(self.after.as_str()))?;
        let rest = lines.get(start..)?;
        rest.iter()
            .enumerate()
            .flat_map(|(at, line)| {
                let tail = match at {
                    0 => line
                        .split_once(self.after.as_str())
                        .map_or("", |(_, after)| after),
                    _ => line.as_str(),
                };
                tail.split_whitespace()
            })
            .find(|word| self.word.fits(word))
            .map(String::from)
    }
}

impl Word {
    fn fits(self, word: &str) -> bool {
        match self {
            Self::Link => word.strip_prefix("https://").is_some_and(|tail| {
                !tail.starts_with(['/', '\\', '?', '#'])
                    && url::Url::parse(word).is_ok()
            }),
            Self::Code => {
                word.len() >= 6
                    && word.bytes().any(|c| c.is_ascii_alphanumeric())
                    && word.chars().all(|c| {
                        c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-'
                    })
            }
        }
    }
}

/// `line` without its terminal escape sequences (`ESC [ … final`), which
/// some CLIs colour their output with even on a pipe.
#[must_use]
pub fn plain(line: &str) -> String {
    let mut text = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            skip_escape(&mut chars);
        } else {
            text.push(c);
        }
    }
    text
}

fn skip_escape(chars: &mut core::str::Chars<'_>) {
    match chars.next() {
        Some('[') => {
            let _ = chars.find(|c| ('@'..='~').contains(c));
        }
        Some(']') => {
            let mut escape = false;
            for c in chars {
                if c == '\u{7}' || (escape && c == '\\') {
                    break;
                }
                escape = c == '\u{1b}';
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::{Failure, Pick, Shown, SignIn, Word, plain};
    use crate::Invocation;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(plain).collect()
    }

    fn device() -> SignIn {
        SignIn {
            run: Invocation::default(),
            link: Pick {
                after: String::from("Open this link"),
                word: Word::Link,
            },
            code: Some(Pick {
                after: String::from("one-time code"),
                word: Word::Code,
            }),
            paste: false,
            failures: Vec::from([Failure {
                says: String::from("timed out"),
                means: String::from("The code expired."),
            }]),
        }
    }

    #[test]
    fn escapes_are_dropped_and_other_text_kept() {
        assert_eq!(
            [
                plain("\u{1b}[94mhttps://example.test/x\u{1b}[0m"),
                plain("a\u{1b}[1;2mb\u{1b}"),
                plain("\u{1b}Mtext"),
                plain("plain…"),
            ],
            [
                String::from("https://example.test/x"),
                String::from("ab"),
                String::from("text"),
                String::from("plain…"),
            ]
        );
    }

    #[test]
    fn a_pick_takes_the_first_word_of_its_shape_after_its_text() {
        let output = lines(
            "Open this link in your browser\n   https://example.test/device\n2. Enter this one-time code (expires in 15 minutes)\n   AB12-CD34E\n",
        );
        assert_eq!(
            device().shown(&output),
            Some(Shown {
                link: String::from("https://example.test/device"),
                code: Some(String::from("AB12-CD34E")),
            })
        );
    }

    #[test]
    fn skips_unusable_candidates_and_strips_terminal_links() {
        let output = lines(concat!(
            "Open this link https:// https:///invalid https://[invalid]\n",
            "\u{1b}]8;;https://invisible.example\u{1b}\\",
            "\u{1b}[94mhttps://example.test/device?session=test\u{1b}[0m",
            "\u{1b}]8;;\u{7}\n",
            "Enter this one-time code ------\nAB12-CD34E\n",
        ));
        assert_eq!(
            device().shown(&output),
            Some(Shown {
                link: String::from("https://example.test/device?session=test"),
                code: Some(String::from("AB12-CD34E")),
            })
        );
    }

    #[test]
    fn nothing_is_shown_until_every_part_is_there() {
        let device = device();
        let partial = lines("Open this link\n  https://example.test/device\n");
        let link_only = SignIn {
            code: None,
            ..device.clone()
        };
        assert_eq!(
            (
                device.shown(&partial),
                device.shown(&lines("https://example.test/device\n")),
                link_only.shown(&partial).map(|shown| shown.link),
            ),
            (
                None,
                None,
                Some(String::from("https://example.test/device"))
            )
        );
    }

    #[test]
    fn a_word_before_the_text_or_of_another_shape_is_not_taken() {
        let pick = Pick {
            after: String::from("code"),
            word: Word::Code,
        };
        assert_eq!(
            [
                pick.find(&lines("ABCDEF-1 code (expires in 15 minutes)\n")),
                pick.find(&lines("code ABCDE abcdef-12 ABC-123\n")),
            ],
            [None, Some(String::from("ABC-123"))]
        );
    }

    #[test]
    fn a_failure_is_known_by_what_the_cli_says() {
        let device = device();
        assert_eq!(
            [
                device.failure("error: device auth timed out after 15 minutes"),
                device.failure("something else"),
            ],
            [Some("The code expired."), None]
        );
    }
}
