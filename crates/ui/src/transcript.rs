//! A transcript item as the pane draws it: messages, reasoning, tool calls
//! with their approval cards, and question cards.

use alloc::collections::BTreeMap;

use crate::prelude::*;
use client::answers::Draft;
use client::host_view::{Approval, Item, ReplyState, RequestState, ToolState};
use proto::{
    Change, Decision, Question, RequestId, Resolution, Selection, ToolKind,
    TurnOutcome,
};

use crate::session::closed;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    User {
        text: String,
        by: String,
    },
    Reply {
        class: &'static str,
        text: String,
    },
    /// Reasoning still streaming in: its summary is the live "Thinking…".
    Thinking(String),
    /// Reasoning done, collapsed under "Thinking".
    Thought(String),
    Continued(String),
    Tool(ToolLine),
    Approval(ApprovalCard),
    Question(QuestionCard),
    TurnEnd {
        outcome: &'static str,
        text: String,
    },
    ModelChanged(String),
    Error(String),
}

/// A tool call's outcome and what it acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolLine {
    pub state: &'static str,
    pub detail: ToolDetail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolDetail {
    Command(String),
    Files(Vec<ToolFile>),
    /// A file change that names no file.
    NoFiles,
    Other {
        name: String,
        detail: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolFile {
    pub verb: &'static str,
    pub path: String,
}

/// A tool call that asked to run. One card for the open and the resolved
/// request, so the focus a button moves to it stays when the buttons go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalCard {
    pub id: String,
    pub open: bool,
    pub request: RequestId,
    pub tool: ToolLine,
    /// Why the agent asks, in its own words.
    pub reason: Option<String>,
    pub ask: Ask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// The decisions offered. Approving for the session allows what
    /// `for_session` says, when the agent offers it.
    Open {
        actions: Vec<Action>,
        for_session: Option<String>,
    },
    Resolved(String),
}

/// A button that decides a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub class: &'static str,
    pub label: &'static str,
    pub decision: Decision,
}

/// Questions the agent asked: the form while open, then the answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionCard {
    pub id: String,
    pub open: bool,
    pub body: QuestionBody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionBody {
    Form(QuestionForm),
    Answered {
        questions: Vec<AnsweredQuestion>,
        resolved: String,
    },
}

/// Each question's options as buttons and a field for any other answer,
/// sent together once each question has an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionForm {
    pub request: RequestId,
    pub fields: Vec<QuestionField>,
    pub send_disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionField {
    pub index: usize,
    pub header: String,
    pub text: String,
    pub hint: &'static str,
    pub options: Vec<OptionButton>,
    pub typed: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionButton {
    pub label: String,
    pub description: String,
    pub pressed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnsweredQuestion {
    pub text: String,
    pub answer: Option<String>,
}

pub fn row(item: &Item, drafts: &BTreeMap<RequestId, Draft>) -> Row {
    match item {
        Item::User { text, by } => Row::User {
            text: text.clone(),
            by: by.0.clone(),
        },
        Item::Reply { text, state } => Row::Reply {
            class: state.class(),
            text: text.clone(),
        },
        Item::Reasoning {
            text,
            state: ReplyState::Streaming,
        } => Row::Thinking(text.clone()),
        Item::Reasoning {
            text,
            state: ReplyState::Done,
        } => Row::Thought(text.clone()),
        Item::Continued(reason) => {
            Row::Continued(format!("Continued after: {}", closed(reason)))
        }
        Item::Tool {
            tool,
            state,
            approval: Approval::NotAsked,
            ..
        } => Row::Tool(tool_line(tool, *state)),
        Item::Tool {
            tool,
            state,
            approval:
                Approval::Asked {
                    request,
                    reason,
                    for_session,
                    state: asked,
                },
            ..
        } => Row::Approval(approval(
            *request,
            tool_line(tool, *state),
            reason.clone(),
            for_session.clone(),
            asked,
        )),
        Item::Question {
            request,
            questions,
            state,
        } => Row::Question(QuestionCard {
            id: card_id(*request),
            open: state.is_open(),
            body: match state {
                RequestState::Open => QuestionBody::Form(form(
                    *request,
                    questions,
                    &drafts
                        .get(request)
                        .cloned()
                        .unwrap_or_else(|| Draft::new(questions)),
                )),
                RequestState::Resolved(resolution) => {
                    answered(questions, resolution)
                }
            },
        }),
        Item::TurnCompleted(outcome) => turn_end(outcome),
        Item::ModelChanged(reason) => Row::ModelChanged(reason.clone()),
        Item::Error(message) => Row::Error(message.clone()),
    }
}

/// A request card's element id, unique in the pane, which shows one
/// session.
pub fn card_id(request: RequestId) -> String {
    format!("request-{}", request.0)
}

fn tool_line(tool: &ToolKind, state: ToolState) -> ToolLine {
    ToolLine {
        state: state.label(),
        detail: match tool {
            ToolKind::Command { command } => {
                ToolDetail::Command(command.clone())
            }
            ToolKind::FileChange { files } if files.is_empty() => {
                ToolDetail::NoFiles
            }
            ToolKind::FileChange { files } => ToolDetail::Files(
                files
                    .iter()
                    .map(|file| ToolFile {
                        verb: verb(file.change),
                        path: file.path.clone(),
                    })
                    .collect(),
            ),
            ToolKind::Other { name, detail } => ToolDetail::Other {
                name: name.clone(),
                detail: detail.clone(),
            },
        },
    }
}

const fn verb(change: Change) -> &'static str {
    match change {
        Change::Write => "Write",
        Change::Edit => "Edit",
        Change::Delete => "Delete",
    }
}

fn form(
    request: RequestId,
    questions: &[Question],
    draft: &Draft,
) -> QuestionForm {
    QuestionForm {
        request,
        fields: questions
            .iter()
            .enumerate()
            .map(|(index, question)| {
                let chosen = draft.chosen(index);
                QuestionField {
                    index,
                    header: question.header.clone(),
                    text: format!(" {}", question.text),
                    hint: match question.selection {
                        Selection::One => "Choose one, or write your own",
                        Selection::Many => "Choose any, and add your own",
                    },
                    options: question
                        .options
                        .iter()
                        .map(|option| OptionButton {
                            label: option.label.clone(),
                            description: option.description.clone(),
                            pressed: chosen.contains(&option.label),
                        })
                        .collect(),
                    typed: draft.typed(index),
                }
            })
            .collect(),
        send_disabled: draft.answers().is_none(),
    }
}

/// A resolved question: each question with its answer, and who answered.
fn answered(questions: &[Question], resolution: &Resolution) -> QuestionBody {
    let answers = match resolution {
        Resolution::Answered { answers, .. } => answers.as_slice(),
        Resolution::Decided { .. } | Resolution::Withdrawn => &[],
    };
    QuestionBody::Answered {
        questions: questions
            .iter()
            .enumerate()
            .map(|(index, question)| AnsweredQuestion {
                text: question.text.clone(),
                answer: answers.get(index).map(|given| given.0.join(", ")),
            })
            .collect(),
        resolved: resolved(resolution),
    }
}

fn resolved(resolution: &Resolution) -> String {
    match resolution {
        Resolution::Decided {
            decision: Decision::Approve,
            by,
        } => format!("Approved by {}", by.0),
        Resolution::Decided {
            decision: Decision::ApproveForSession,
            by,
        } => format!("Allowed for this session by {}", by.0),
        Resolution::Decided {
            decision: Decision::Deny,
            by,
        } => format!("Denied by {}", by.0),
        Resolution::Answered { by, .. } => format!("Answered by {}", by.0),
        Resolution::Withdrawn => String::from("Withdrawn"),
    }
}

/// The card of a tool call that asked to run: the decisions while open,
/// then who decided.
fn approval(
    request: RequestId,
    tool: ToolLine,
    reason: Option<String>,
    for_session: Option<String>,
    state: &RequestState,
) -> ApprovalCard {
    ApprovalCard {
        id: card_id(request),
        open: state.is_open(),
        request,
        tool,
        reason,
        ask: match state {
            RequestState::Open => Ask::Open {
                actions: Vec::from([
                    Action {
                        class: "deny",
                        label: "Deny",
                        decision: Decision::Deny,
                    },
                    Action {
                        class: "approve",
                        label: "Approve",
                        decision: Decision::Approve,
                    },
                ]),
                for_session,
            },
            RequestState::Resolved(resolution) => {
                Ask::Resolved(resolved(resolution))
            }
        },
    }
}

fn turn_end(outcome: &TurnOutcome) -> Row {
    match outcome {
        TurnOutcome::Completed => Row::TurnEnd {
            outcome: "completed",
            text: String::from("Turn finished"),
        },
        TurnOutcome::Interrupted => Row::TurnEnd {
            outcome: "interrupted",
            text: String::from("Interrupted"),
        },
        TurnOutcome::Failed { message } => Row::TurnEnd {
            outcome: "failed",
            text: format!("Failed: {message}"),
        },
    }
}
