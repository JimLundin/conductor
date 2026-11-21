//! One session: its header, transcript and composer, and the drafts and
//! diffs the pane holds while it is shown.

use alloc::collections::BTreeMap;

use crate::prelude::*;
use client::host_view::{
    self, EmptyTranscript, Interrupt, Item, Resume, SessionView, StatusLabel,
    UndoState, Working,
};
use proto::{
    Answer, Availability, Catalog, CheckpointId, CloseReason, CommandBody,
    CommandResult, Decision, Effort, Instances, Models, Question, RequestId,
    Selection, SessionId, Task, TaskStatus,
};

use crate::checkpoints::{self, DiffState, TurnChanges};
use crate::new_session::Choice;
use crate::sessions::{agent, short};
use crate::transcript::{self, AgentGroup, Row, card_id};
use crate::{At, Effect, Focus, ReplyTo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionIntent {
    TypeMessage(String),
    SendMessage,
    Interrupt,
    /// Start a closed session's agent again.
    Continue,
    /// Remove a closed session and its workspace, keeping its branch.
    Remove,
    /// Finish a session on an issue: summary, note, branch landed.
    Finish,
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
    /// The session works on an issue, runs no turn and has done something
    /// since it last finished, so it can finish.
    pub finish: bool,
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
    /// `resume`: the session takes a `Continue` or a removal.
    Closed {
        text: String,
        resume: bool,
    },
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
        &mut self,
        intent: SessionIntent,
        session: SessionId,
        view: &SessionView,
        at: At,
    ) -> Vec<Effect> {
        let command = match intent {
            SessionIntent::TypeMessage(draft) => {
                self.draft = draft;
                return Vec::new();
            }
            SessionIntent::SendMessage => {
                let text = self.draft.trim();
                if view.controls().composer != host_view::Composer::Open
                    || text.is_empty()
                {
                    return Vec::new();
                }
                let command = CommandBody::SendMessage {
                    session,
                    text: text.to_owned(),
                };
                self.draft.clear();
                command
            }
            SessionIntent::Interrupt => CommandBody::Interrupt { session },
            SessionIntent::Continue => {
                self.focus_composer = true;
                CommandBody::Continue { session }
            }
            SessionIntent::Remove => CommandBody::RemoveSession { session },
            SessionIntent::Finish => CommandBody::FinishSession { session },
            SessionIntent::Undo(checkpoint) => CommandBody::UndoTurn {
                session,
                checkpoint,
            },
            SessionIntent::PickEffort(word) => {
                return set_effort(&word, session, view.effort, at);
            }
            SessionIntent::Decide(request, decision) => {
                return Vec::from([
                    notify(
                        CommandBody::Resolve {
                            session,
                            request,
                            decision,
                        },
                        at,
                    ),
                    focus_card(request),
                ]);
            }
            SessionIntent::Choose {
                request,
                question,
                label,
            } => {
                if let Some((field, selection)) =
                    self.question(view, request, question)
                {
                    field.pick(label, selection);
                }
                return Vec::new();
            }
            SessionIntent::TypeAnswer {
                request,
                question,
                text,
            } => {
                if let Some((field, selection)) =
                    self.question(view, request, question)
                {
                    field.r#type(text, selection);
                }
                return Vec::new();
            }
            SessionIntent::SendAnswer(request) => {
                return self.send_answer(request, session, view, at);
            }
            SessionIntent::ShowChanges(checkpoint) => {
                return self.load_diff(session, checkpoint, at);
            }
            SessionIntent::CloseChanges(checkpoint) => {
                self.diffs.insert(checkpoint, DiffState::Hidden);
                return Vec::from([Effect::Focus(Focus::NextFrame(
                    checkpoints::changes_id(checkpoint),
                ))]);
            }
            SessionIntent::Mounted(mount) => return self.mounted(mount),
        };
        Vec::from([notify(command, at)])
    }

    /// Sends the answers once every question has one.
    fn send_answer(
        &self,
        request: RequestId,
        id: SessionId,
        session: &SessionView,
        at: At,
    ) -> Vec<Effect> {
        let Some(asked) = questions(session, request) else {
            return Vec::new();
        };
        self.answers
            .get(&request)
            .map_or_else(|| Draft::new(asked).answers(), Draft::answers)
            .map_or_else(Vec::new, |answers| {
                Vec::from([
                    notify(
                        CommandBody::Answer {
                            session: id,
                            request,
                            answers,
                        },
                        at,
                    ),
                    focus_card(request),
                ])
            })
    }

    fn mounted(&mut self, mount: Mount) -> Vec<Effect> {
        match mount {
            Mount::Composer if core::mem::take(&mut self.focus_composer) => {
                Vec::from([Effect::Focus(Focus::Composer)])
            }
            Mount::Composer => Vec::new(),
            Mount::Undone => Vec::from([Effect::Focus(Focus::AfterUndo)]),
        }
    }

    fn question(
        &mut self,
        view: &SessionView,
        request: RequestId,
        index: usize,
    ) -> Option<(&mut QuestionDraft, Selection)> {
        let asked = questions(view, request)?;
        let selection = asked.get(index)?.selection;
        let draft = self
            .answers
            .entry(request)
            .or_insert_with(|| Draft::new(asked));
        Some((draft.0.get_mut(index)?, selection))
    }

    fn load_diff(
        &mut self,
        session: SessionId,
        checkpoint: CheckpointId,
        at: At,
    ) -> Vec<Effect> {
        if matches!(self.diffs.get(&checkpoint), Some(DiffState::Loading(_))) {
            return Vec::new();
        }
        self.requests = self.requests.wrapping_add(1);
        let request = self.requests;
        self.diffs.insert(checkpoint, DiffState::Loading(request));
        Vec::from([at.send(
            CommandBody::GetDiff {
                session,
                checkpoint,
            },
            ReplyTo::Diff {
                checkpoint,
                request,
            },
        )])
    }

    pub fn diffed(
        &mut self,
        checkpoint: CheckpointId,
        request: u64,
        answer: Result<CommandResult, String>,
        host: &str,
    ) {
        if self.diffs.get(&checkpoint) == Some(&DiffState::Loading(request)) {
            self.diffs
                .insert(checkpoint, DiffState::answered(answer, host));
        }
    }

    pub fn screen(
        &self,
        id: SessionId,
        session: &SessionView,
        known: (Option<&Instances>, &Catalog),
        (notice, host): (Option<String>, String),
    ) -> SessionPane {
        let controls = session.controls();
        let provider = agent(session, known);
        let status = String::from(status_text(session.status_label()));
        let offered = known
            .0
            .and_then(|listed| listed.availability(&session.instance))
            .map_or_else(Models::none, Availability::models)
            .efforts(session.model.as_ref());
        SessionPane {
            key: format!("{}", id.0),
            title: short(&session.directory).to_owned(),
            subtitle: format!("{} on {host}", session.directory.0),
            pill: format!("{provider}, {status}"),
            model: session.model.as_ref().map_or_else(
                || String::from("default model"),
                |model| model.0.clone(),
            ),
            effort: effort_control(session.effort, &offered),
            interrupt: controls.interrupt == Interrupt::Offered,
            finish: controls.finish,
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
            empty: session
                .empty_transcript()
                .map(|kind| empty(kind, &provider)),
            entries: self.entries(session),
            working: session.working() == Working::Shown,
            notice,
            footer: session.closed_reason().map_or_else(
                || {
                    Footer::Composer(Composer {
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
                        disabled: controls.composer
                            != host_view::Composer::Open,
                    })
                },
                |reason| Footer::Closed {
                    text: closed(reason),
                    resume: controls.resume == Resume::Offered,
                },
            ),
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
                            session.can_view_changes(),
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

fn empty(kind: EmptyTranscript, provider: &str) -> String {
    match kind {
        EmptyTranscript::Starting => format!("Starting {provider}…"),
        EmptyTranscript::Ready => {
            format!("{provider} is ready. Send a message to begin.")
        }
        EmptyTranscript::Closed => {
            String::from("This session ended before it began.")
        }
    }
}

pub const fn status_text(label: StatusLabel) -> &'static str {
    match label {
        StatusLabel::Starting => "starting",
        StatusLabel::Idle => "idle",
        StatusLabel::Running => "running",
        StatusLabel::Closed => "closed",
        StatusLabel::Undoing => "undoing",
        StatusLabel::RecoveryNeeded => "recovery needed",
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
const fn notify(body: CommandBody, at: At) -> Effect {
    at.send(body, ReplyTo::Notice)
}

/// Sends `SetEffort`. A refusal puts the picker back on `shown`, the
/// effort the session still has.
fn set_effort(
    word: &str,
    id: SessionId,
    shown: Option<Effort>,
    at: At,
) -> Vec<Effect> {
    Effort::parse(word)
        .map(|effort| {
            at.send(
                CommandBody::SetEffort {
                    session: id,
                    effort,
                },
                ReplyTo::Effort(shown),
            )
        })
        .into_iter()
        .collect()
}

/// For each question, the options chosen and the text typed so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft(Vec<QuestionDraft>);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct QuestionDraft {
    chosen: Vec<String>,
    typed: String,
}

impl QuestionDraft {
    fn pick(&mut self, label: String, selection: Selection) {
        match selection {
            Selection::One => {
                self.chosen.clear();
                self.typed.clear();
            }
            Selection::Many => {
                if let Some(index) =
                    self.chosen.iter().position(|old| *old == label)
                {
                    self.chosen.remove(index);
                    return;
                }
            }
        }
        self.chosen.push(label);
    }

    fn r#type(&mut self, text: String, selection: Selection) {
        self.typed = text;
        if selection == Selection::One {
            self.chosen.clear();
        }
    }

    fn answer(&self) -> Option<Answer> {
        let mut labels = self.chosen.clone();
        let text = self.typed.trim();
        if !text.is_empty() {
            labels.push(text.to_owned());
        }
        (!labels.is_empty()).then_some(Answer(labels))
    }
}

impl Draft {
    pub(crate) fn new(questions: &[Question]) -> Self {
        Self(alloc::vec![QuestionDraft::default(); questions.len()])
    }

    pub(crate) fn chosen(&self, index: usize) -> &[String] {
        self.0
            .get(index)
            .map_or(&[], |field| field.chosen.as_slice())
    }

    pub(crate) fn typed(&self, index: usize) -> &str {
        self.0.get(index).map_or("", |field| field.typed.as_str())
    }

    pub(crate) fn answers(&self) -> Option<Vec<Answer>> {
        self.0.iter().map(QuestionDraft::answer).collect()
    }
}

#[cfg(test)]
mod tests {
    use proto::{Answer, Question, Selection};

    use super::Draft;

    fn question(selection: Selection) -> Question {
        Question {
            header: String::from("Q"),
            text: String::from("Which?"),
            options: Vec::new(),
            selection,
        }
    }

    fn answer(words: &[&str]) -> Answer {
        Answer(words.iter().copied().map(String::from).collect())
    }

    #[test]
    fn every_question_needs_an_answer_before_the_draft_is_sent() {
        let mut draft =
            Draft::new(&[question(Selection::One), question(Selection::Many)]);
        assert_eq!(draft.answers(), None);
        draft.0[0].pick(String::from("Red"), Selection::One);
        assert_eq!(draft.answers(), None);
        draft.0[0].pick(String::from("Blue"), Selection::One);
        for label in ["Apple", "Pear", "Apple"] {
            draft.0[1].pick(String::from(label), Selection::Many);
        }
        draft.0[1].r#type(String::from(" Plum "), Selection::Many);
        assert_eq!(
            draft.answers(),
            Some(Vec::from([answer(&["Blue"]), answer(&["Pear", "Plum"])]))
        );
        draft.0[0].r#type(String::from("Green"), Selection::One);
        assert_eq!(
            draft.answers(),
            Some(Vec::from([answer(&["Green"]), answer(&["Pear", "Plum"])]))
        );
        draft.0[0].pick(String::from("Red"), Selection::One);
        assert_eq!(
            draft.answers(),
            Some(Vec::from([answer(&["Red"]), answer(&["Pear", "Plum"])]))
        );
        assert_eq!(draft.typed(0), "");
    }
    #[test]
    fn the_typed_text_is_kept_per_question() {
        let mut draft =
            Draft::new(&[question(Selection::One), question(Selection::One)]);
        draft.0[1].r#type(String::from("Green"), Selection::One);
        assert_eq!(
            [draft.typed(0), draft.typed(1), draft.typed(2)],
            ["", "Green", ""]
        );
    }
}
