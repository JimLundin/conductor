//! What a core decides is worth logging, as data: the shell logs it.

use crate::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub level: Level,
    pub message: String,
}

impl Note {
    #[must_use]
    pub const fn error(message: String) -> Self {
        Self {
            level: Level::Error,
            message,
        }
    }

    #[must_use]
    pub const fn warn(message: String) -> Self {
        Self {
            level: Level::Warn,
            message,
        }
    }

    #[must_use]
    pub const fn info(message: String) -> Self {
        Self {
            level: Level::Info,
            message,
        }
    }

    #[must_use]
    pub const fn debug(message: String) -> Self {
        Self {
            level: Level::Debug,
            message,
        }
    }

    #[must_use]
    pub const fn trace(message: String) -> Self {
        Self {
            level: Level::Trace,
            message,
        }
    }
}
