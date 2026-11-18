//! One session: its header, transcript and composer, and the drafts and
//! diffs the pane holds while it is shown.

use alloc::collections::BTreeMap;

use crate::prelude::*;
use client::answers::Draft;
use client::host_view::{
    self, Interrupt, Item, Resume, SessionStatus, SessionView, UndoState,
    Working,
};
use client::new_session::{efforts, models};
use proto::{
    Catalog, CheckpointId, CloseReason, CommandBody, CommandResult, Decision,
    Effort, Instances, Question, RequestId, SessionId, Task, TaskStatus,
};

use crate::checkpoints::{self, DiffState, TurnChanges};
use crate::new_session::Choice;
use crate::sessions::{agent, short};
use crate::transcript::{self, AgentGroup, Row, card_id};
use crate::{Effect, Focus, Reply, ReplyTo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionIntent {
    TypeMessage(String),
    SendMessage,
    Interrupt,
    /// Start a closed session's agent again.
    Continue,
    /// An effort's word, from the next turn on.
    PickEffort(String),
    Decide(RequestId, Decision),
    /// Tap an option of a question.
    Choose {
        request: RequestId,
        question: usize,
        label: String,
    },
    TypeAnswer {
        request: RequestId,
        question: usize,
        text: String,
    },
    SendAnswer(RequestId),
    ShowChanges(CheckpointId),
    CloseChanges(CheckpointId),
    /// Undo this turn and every later one, or retry an undo that failed.
    Undo(CheckpointId),
    Mounted(Mount),
}

/// An element that just appeared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mount {
    Composer,
    /// The note that a turn was undone.
    Undone,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPane {
    pub key: String,
    /// The session's state in a word, also its `data-status`.
    pub status: String,
    pub title: String,
    pub subtitle: String,
    pub pill: String,
    pub model: String,
    pub effort: EffortControl,
    pub interrupt: bool,
    pub workspace: WorkspaceContext,
    pub undo: UndoNote,
    pub tasks: Option<Tasks>,
    /// What an empty transcript says.
    pub empty: Option<String>,
    pub entries: Vec<Entry>,
    /// The transcript ends with a sign that the agent is working.
    pub working: bool,
    pub notice: Option<String>,
    pub footer: Footer,
}

/// The session's effort, which changes from the next turn on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffortControl {
    Hidden,
    /// The effort alone, when the host lists none to pick from.
    Fixed(String),
    Picker(Vec<Choice>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceContext {
    Plain(&'static str),
    Branch { branch: String, directory: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UndoNote {
    Hidden,
    Undoing(&'static str),
    /// The undo failed; retrying sends the same checkpoint.
    Failed {
        text: String,
        checkpoint: CheckpointId,
    },
    Undone {
        key: String,
        text: &'static str,
    },
}

/// The agent's checklist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tasks {
    pub count: String,
    pub items: Vec<TaskRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
    pub key: String,
    pub status: &'static str,
    pub label: &'static str,
    pub text: String,
}

/// A transcript item, the sub-agents its tool call started, and the
/// changes of the turn it ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub row: Row,
    pub agents: Vec<AgentGroup>,
    pub changes: Option<TurnChanges>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Footer {
    Closed { text: String, resume: bool },
    Composer(Composer),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Composer {
    pub text: String,
    pub placeholder: String,
    pub disabled: bool,
}

/// What the pane holds while one session is shown.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Pane {
    draft: String,
    answers: BTreeMap<RequestId, Draft>,
    diffs: BTreeMap<CheckpointId, DiffState>,
    /// Numbers each diff request, so only the latest one's answer shows.
    requests: u64,
    /// Continue took the button away; the composer that replaces it takes
    /// the focus once the agent is ready again.
    focus_composer: bool,
}

impl Pane {
    pub fn update(
        self,
        intent: SessionIntent,
        id: SessionId,
        session: &SessionView,
        visit: u64,
    ) -> (Self, Vec<Effect>) {
        match intent {
            SessionIntent::TypeMessage(draft) => {
                (Self { draft, ..self }, Vec::new())
            }
            SessionIntent::SendMessage => self.send_message(id, session, visit),
            SessionIntent::Interrupt => (
                self,
                Vec::from([notify(
                    CommandBody::Interrupt { session: id },
                    visit,
                )]),
            ),
            SessionIntent::Continue => (
                Self {
                    focus_composer: true,
                    ..self
                },
                Vec::from([notify(
                    CommandBody::Continue { session: id },
                    visit,
                )]),
            ),
            SessionIntent::PickEffort(word) => {
                (self, set_effort(&word, id, session.effort, visit))
            }
            SessionIntent::Decide(request, decision) => (
                self,
                Vec::from([
                    notify(
                        CommandBody::Resolve {
                            session: id,
                            request,
                            decision,
                        },
                        visit,
                    ),
                    focus_card(request),
                ]),
            ),
            SessionIntent::Choose {
                request,
                question,
                label,
            } => (
                self.drafted(session, request, question, |draft, selection| {
                    draft.pick(question, label, selection)
                }),
                Vec::new(),
            ),
            SessionIntent::TypeAnswer {
                request,
                question,
                text,
            } => (
                self.drafted(session, request, question, |draft, selection| {
                    draft.r#type(question, &text, selection)
                }),
                Vec::new(),
            ),
            SessionIntent::SendAnswer(request) => {
                let effects = self.send_answer(request, id, session, visit);
                (self, effects)
            }
            SessionIntent::ShowChanges(checkpoint) => {
                self.load_diff(id, checkpoint, visit)
            }
            SessionIntent::CloseChanges(checkpoint) => {
                self.close_diff(checkpoint)
            }
            SessionIntent::Undo(checkpoint) => (
                self,
                Vec::from([notify(
                    CommandBody::UndoTurn {
                        session: id,
                        checkpoint,
                    },
                    visit,
                )]),
            ),
            SessionIntent::Mounted(mount) => self.mounted(mount),
        }
    }

    /// Sends the draft, once there is one and the agent takes messages.
    fn send_message(
        self,
        id: SessionId,
        session: &SessionView,
        visit: u64,
    ) -> (Self, Vec<Effect>) {
        match (session.controls().composer, self.draft.trim()) {
            (host_view::Composer::Open, "")
            | (host_view::Composer::Busy | host_view::Composer::Closed, _) => {
                (self, Vec::new())
            }
            (host_view::Composer::Open, text) => {
                let body = CommandBody::SendMessage {
                    session: id,
                    text: text.to_owned(),
                };
                (
                    Self {
                        draft: String::new(),
                        ..self
                    },
                    Vec::from([notify(body, visit)]),
                )
            }
        }
    }

    /// Sends the answers once every question has one.
    fn send_answer(
        &self,
        request: RequestId,
        id: SessionId,
        session: &SessionView,
        visit: u64,
    ) -> Vec<Effect> {
        self.draft_for(session, request)
            .and_then(|draft| draft.answers())
            .map_or_else(Vec::new, |answers| {
                Vec::from([
                    notify(
                        CommandBody::Answer {
                            session: id,
                            request,
                            answers,
                        },
                        visit,
                    ),
                    focus_card(request),
                ])
            })
    }

    fn close_diff(self, checkpoint: CheckpointId) -> (Self, Vec<Effect>) {
        let mut diffs = self.diffs;
        diffs.insert(checkpoint, DiffState::Hidden);
        (
            Self { diffs, ..self },
            Vec::from([Effect::Focus(Focus::NextFrame(
                checkpoints::changes_id(checkpoint),
            ))]),
        )
    }

    fn mounted(self, mount: Mount) -> (Self, Vec<Effect>) {
        match mount {
            Mount::Composer if self.focus_composer => (
                Self {
                    focus_composer: false,
                    ..self
                },
                Vec::from([Effect::Focus(Focus::Composer)]),
            ),
            Mount::Composer => (self, Vec::new()),
            Mount::Undone => {
                (self, Vec::from([Effect::Focus(Focus::AfterUndo)]))
            }
        }
    }

    /// The draft of a question card, a fresh one until the person starts
    /// answering. `None` when the session asked no such question.
    fn draft_for(
        &self,
        session: &SessionView,
        request: RequestId,
    ) -> Option<Draft> {
        questions(session, request).map(|asked| {
            self.answers
                .get(&request)
                .cloned()
                .unwrap_or_else(|| Draft::new(asked))
        })
    }

    fn drafted(
        self,
        session: &SessionView,
        request: RequestId,
        question: usize,
        change: impl FnOnce(Draft, proto::Selection) -> Draft,
    ) -> Self {
        let selection = questions(session, request)
            .and_then(|asked| asked.get(question))
            .map(|asked| asked.selection);
        match (self.draft_for(session, request), selection) {
            (Some(draft), Some(kind)) => {
                let mut answers = self.answers;
                answers.insert(request, change(draft, kind));
                Self { answers, ..self }
            }
            (None, _) | (_, None) => self,
        }
    }

    fn load_diff(
        self,
        session: SessionId,
        checkpoint: CheckpointId,
        visit: u64,
    ) -> (Self, Vec<Effect>) {
        if let Some(DiffState::Loading(_)) = self.diffs.get(&checkpoint) {
            return (self, Vec::new());
        }
        let request = self.requests.wrapping_add(1);
        let mut diffs = self.diffs;
        diffs.insert(checkpoint, DiffState::Loading(request));
        (
            Self {
                diffs,
                requests: request,
                ..self
            },
            Vec::from([Effect::Send {
                body: CommandBody::GetDiff {
                    session,
                    checkpoint,
                },
                reply: Reply {
                    visit,
                    to: ReplyTo::Diff {
                        checkpoint,
                        request,
                    },
                },
            }]),
        )
    }

    /// A diff's answer, unless a later request or a close replaced it.
    pub fn diffed(
        self,
        checkpoint: CheckpointId,
        request: u64,
        answer: Result<CommandResult, String>,
    ) -> Self {
        if self.diffs.get(&checkpoint) != Some(&DiffState::Loading(request)) {
            return self;
        }
        let mut diffs = self.diffs;
        diffs.insert(checkpoint, DiffState::answered(answer));
        Self { diffs, ..self }
    }

    pub fn screen(
        &self,
        id: SessionId,
        session: &SessionView,
        known: (Option<&Instances>, &Catalog),
        notice: Option<String>,
    ) -> SessionPane {
        let controls = session.controls();
        let provider = agent(session, known);
        let status = match session.undo {
            UndoState::Undoing(_) => String::from("undoing"),
            UndoState::Failed { .. } => String::from("recovery needed"),
            UndoState::Ready | UndoState::Undone => session.status.word(),
        };
        let offered = efforts(
            &models(
                known
                    .0
                    .and_then(|listed| listed.availability(&session.instance)),
            ),
            session.model.as_ref(),
        );
        SessionPane {
            key: format!("{}", id.0),
            title: short(&session.directory).to_owned(),
            subtitle: session.directory.0.clone(),
            pill: format!("{provider}, {status}"),
            model: session.model.as_ref().map_or_else(
                || String::from("default model"),
                |model| model.0.clone(),
            ),
            effort: effort_control(session.effort, &offered),
            interrupt: controls.interrupt == Interrupt::Offered,
            workspace: session.workspace.as_ref().map_or(
                WorkspaceContext::Plain(
                    "This session has no worktree checkpoints.",
                ),
                |workspace| WorkspaceContext::Branch {
                    branch: format!("Branch: {}", workspace.branch),
                    directory: workspace.directory.0.clone(),
                },
            ),
            undo: undo_note(id, &session.undo),
            tasks: tasks(&session.tasks),
            empty: empty(&session.status, session.items.is_empty(), &provider),
            entries: self.entries(session),
            working: session.working() == Working::Shown,
            notice,
            footer: match &session.status {
                SessionStatus::Closed(reason) => Footer::Closed {
                    text: closed(reason),
                    resume: controls.resume == Resume::Offered,
                },
                SessionStatus::Starting
                | SessionStatus::Idle
                | SessionStatus::Running => Footer::Composer(Composer {
                    text: self.draft.clone(),
                    placeholder: match controls.composer {
                        host_view::Composer::Open => {
                            format!("Message {provider}")
                        }
                        host_view::Composer::Busy => {
                            format!("{provider} is working…")
                        }
                        host_view::Composer::Closed => {
                            String::from("This session has ended")
                        }
                    },
                    disabled: controls.composer != host_view::Composer::Open,
                }),
            },
            status,
        }
    }

    fn entries(&self, session: &SessionView) -> Vec<Entry> {
        let composer = session.controls().composer;
        session
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| Entry {
                key: format!("{index}"),
                row: transcript::row(item, &self.answers),
                agents: item.call().map_or_else(Vec::new, |call| {
                    session
                        .subagents
                        .iter()
                        .filter(|subagent| &subagent.call == call)
                        .map(|subagent| {
                            transcript::agent_group(subagent, &self.answers)
                        })
                        .collect()
                }),
                changes: session
                    .checkpoints
                    .iter()
                    .find(|turn| turn.end == index)
                    .map(|turn| {
                        checkpoints::changes(
                            turn,
                            self.diffs.get(&turn.checkpoint),
                            composer,
                            &session.undo,
                        )
                    }),
            })
            .collect()
    }
}

fn effort_control(effort: Option<Effort>, offered: &[Effort]) -> EffortControl {
    match (offered.first(), effort) {
        (None, Some(current)) => {
            EffortControl::Fixed(format!("{} effort", current.wire()))
        }
        (None, None) => EffortControl::Hidden,
        (Some(_), _) => {
            let unset = effort.is_none().then(|| Choice {
                value: String::new(),
                label: String::from("default effort"),
                selected: true,
                disabled: true,
            });
            let listed = offered.iter().map(|choice| Choice {
                value: choice.wire().to_owned(),
                label: format!("{} effort", choice.wire()),
                selected: Some(*choice) == effort,
                disabled: false,
            });
            EffortControl::Picker(unset.into_iter().chain(listed).collect())
        }
    }
}

fn undo_note(id: SessionId, undo: &UndoState) -> UndoNote {
    match undo {
        UndoState::Undoing(_) => UndoNote::Undoing(
            "Undoing this turn and all later file and conversation changes…",
        ),
        UndoState::Failed { checkpoint, reason } => UndoNote::Failed {
            text: format!(
                "Undo needs recovery: {reason}. Other actions are blocked until recovery finishes."
            ),
            checkpoint: *checkpoint,
        },
        UndoState::Undone => UndoNote::Undone {
            key: format!("undo-done-{}", id.0),
            text: "Turn and all later file and conversation changes undone.",
        },
        UndoState::Ready => UndoNote::Hidden,
    }
}

/// The agent's checklist, as the session holds it.
fn tasks(tasks: &[Task]) -> Option<Tasks> {
    let done = tasks
        .iter()
        .filter(|task| task.status == TaskStatus::Completed)
        .count();
    tasks.first().map(|_| Tasks {
        count: format!("{done} of {} done", tasks.len()),
        items: tasks
            .iter()
            .map(|task| TaskRow {
                key: format!("{}", task.id.0),
                status: match task.status {
                    TaskStatus::Pending => "pending",
                    TaskStatus::InProgress => "in-progress",
                    TaskStatus::Completed => "completed",
                },
                label: match task.status {
                    TaskStatus::Pending => "To do",
                    TaskStatus::InProgress => "In progress",
                    TaskStatus::Completed => "Done",
                },
                text: task.text.clone(),
            })
            .collect(),
    })
}

fn empty(
    status: &SessionStatus,
    no_items: bool,
    provider: &str,
) -> Option<String> {
    match (status, no_items) {
        (_, false) => None,
        (SessionStatus::Starting, true) => {
            Some(format!("Starting {provider}…"))
        }
        (SessionStatus::Idle | SessionStatus::Running, true) => {
            Some(format!("{provider} is ready. Send a message to begin."))
        }
        (SessionStatus::Closed(_), true) => {
            Some(String::from("This session ended before it began."))
        }
    }
}

pub fn closed(close: &CloseReason) -> String {
    match close {
        CloseReason::FailedToStart { message } => {
            format!("The agent did not start: {message}")
        }
        CloseReason::Exited { status } => {
            format!("The session has ended ({status}).")
        }
        CloseReason::Stopped { reason } => format!("Stopped: {reason}"),
    }
}

fn questions(session: &SessionView, request: RequestId) -> Option<&[Question]> {
    session.items.iter().find_map(|item| match item {
        Item::Question {
            request: asked,
            questions,
            ..
        } if *asked == request => Some(questions.as_slice()),
        _ => None,
    })
}

/// Moves focus to a request's card, which stays when its buttons go.
fn focus_card(request: RequestId) -> Effect {
    Effect::Focus(Focus::Element(card_id(request)))
}

/// A command whose only answer worth showing is a refusal.
const fn notify(body: CommandBody, visit: u64) -> Effect {
    Effect::Send {
        body,
        reply: Reply {
            visit,
            to: ReplyTo::Notice,
        },
    }
}

/// Sends `SetEffort`. A refusal puts the picker back on `shown`, the
/// effort the session still has.
fn set_effort(
    word: &str,
    id: SessionId,
    shown: Option<Effort>,
    visit: u64,
) -> Vec<Effect> {
    Effort::parse(word)
        .map(|effort| Effect::Send {
            body: CommandBody::SetEffort {
                session: id,
                effort,
            },
            reply: Reply {
                visit,
                to: ReplyTo::Effort(shown),
            },
        })
        .into_iter()
        .collect()
}
