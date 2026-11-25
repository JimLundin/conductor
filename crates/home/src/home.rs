use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::canvas::{CanvasFile, CanvasName};
use crate::flow::Flow;
use crate::issue::{EditId, IssueChange, NewIssue, Observation};
use crate::{IssueId, ProjectId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Home {
    pub remote: String,
    #[serde(rename = "synced")]
    pub synced_secs: Option<u64>,
    pub sync: HomeSync,
    pub unpushed: u64,
    pub flow: Option<Box<Flow>>,
    pub projects: Vec<Project>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum HomeSync {
    Syncing,
    Synced,
    Behind { reason: String },
    Failed { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub title: String,
    pub remote: Option<String>,
    pub profiles: Vec<String>,
    pub provides: Vec<String>,
    pub checkouts: Vec<Checkout>,
    pub cloning: Cloning,
    pub issues: Vec<Observation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Checkout {
    pub path: CheckoutLocation,
    pub origin: Origin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CheckoutLocation(pub String);

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub enum Origin {
    Found,
    Cloned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Cloning {
    Idle,
    Running,
    Failed { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum HomeCommand {
    Sync,
    Clone {
        project: ProjectId,
    },
    CreateIssue {
        edit: EditId,
        issue: NewIssue,
    },
    ChangeIssue {
        edit: EditId,
        issue: IssueId,
        change: IssueChange,
    },
    Canvas {
        issue: IssueId,
        name: CanvasName,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum HomeResult {
    Accepted,
    IssueWritten {
        issue: IssueId,
        warnings: Vec<String>,
    },
    Canvas(CanvasFile),
    Refused(HomeRefusal),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum HomeRefusal {
    NoHome,
    UnknownProject,
    NotCheckedOut,
    NoRemote,
    UnknownIssue,
    IssueNotSaved { reason: String },
    IssueRefused { reason: String },
    UnknownCanvas,
    CanvasUnavailable { reason: String },
}

impl HomeRefusal {
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::NoHome => String::from(
                "the host has no home; start it with --home <remote>",
            ),
            Self::UnknownProject => {
                String::from("the host's home lists no such project")
            }
            Self::NotCheckedOut => String::from(
                "the project isn't checked out on the host; clone it there first",
            ),
            Self::NoRemote => String::from(
                "the project is local only, so there is nothing to clone",
            ),
            Self::UnknownIssue => {
                String::from("the host's home lists no such issue")
            }
            Self::IssueNotSaved { reason } => {
                format!("the issue was not saved: {reason}")
            }
            Self::IssueRefused { reason } => reason.clone(),
            Self::UnknownCanvas => {
                String::from("the issue has no canvas of that name")
            }
            Self::CanvasUnavailable { reason } => {
                format!("the canvas can't be read now: {reason}")
            }
        }
    }
}
