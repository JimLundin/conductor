use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use super::CanvasError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held<T> {
    pub canvases: Vec<T>,
    pub strays: Vec<Stray>,
}

impl<T> Default for Held<T> {
    fn default() -> Self {
        Self {
            canvases: Vec::new(),
            strays: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stray {
    pub place: String,
    pub reason: StrayReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StrayReason {
    Unnamed,
    NotAnIssue,
    Invalid(CanvasError),
}

impl fmt::Display for StrayReason {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unnamed => out.write_str(
                "isn't a canvas: name it in lowercase words joined by `-`, or move it out of the issue",
            ),
            Self::NotAnIssue => out.write_str(
                "isn't an issue: keep canvases under an issue's id, or remove it",
            ),
            Self::Invalid(error) => error.fmt(out),
        }
    }
}
