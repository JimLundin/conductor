use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{Lookup, ReadFailure};
use crate::{UnixMillis, prelude::*};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub id: String,
    pub url: String,
    pub body: String,
    pub updated: UnixMillis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommentWrite {
    Create { body: String },
    Edit { comment: String, body: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentRequest {
    pub issue: Lookup,
    pub write: CommentWrite,
    pub seen: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendFailure {
    Rejected(ReadFailure),
    Invalid,
    Unknown,
}

impl CommentWrite {
    #[must_use]
    pub fn body(&self) -> &str {
        match self {
            Self::Create { body } | Self::Edit { body, .. } => body,
        }
    }

    #[must_use]
    pub fn covers(&self, comment: &Comment) -> bool {
        match self {
            Self::Create { .. } => true,
            Self::Edit { comment: id, .. } => comment.id == *id,
        }
    }

    #[must_use]
    pub fn precondition(&self, comments: &[Comment]) -> String {
        let mut hash = Sha256::new();
        for comment in comments.iter().filter(|comment| self.covers(comment)) {
            for field in [comment.id.as_str(), &comment.url, &comment.body] {
                hash.update((field.len() as u64).to_be_bytes());
                hash.update(field.as_bytes());
            }
            hash.update(comment.updated.0.to_be_bytes());
        }
        data_encoding::HEXLOWER.encode(&hash.finalize())
    }
}
