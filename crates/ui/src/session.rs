//! One session: its header, transcript and composer, and the drafts and
//! diffs the pane holds while it is shown.

use alloc::collections::BTreeMap;

use crate::prelude::*;
use client::host_view::{
    self, EmptyTranscript, Interrupt, Item, Resume, SessionView, StatusLabel,
    Working,
};
use proto::{
    Answer, Availability, Catalog, ChecklistEntry, ChecklistStatus,
    CloseReason, CommandBody, CommandResult, Decision, Effort, HostSeq,
    Instances, Models, Question, RequestId, Selection, SessionId,
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
    RetryMessage,
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
    ShowChanges(HostSeq),
    CloseChanges(HostSeq),
    Mounted(Mount),
}

/// An element that just appeared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mount {
    Composer,
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
    pub tasks: Option<Tasks>,
    /// What an empty transcript says.
    pub empty: Option<String>,
    pub entries: Vec<Entry>,
    pub other_agents: Vec<AgentGroup>,
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

/// The agent's checklist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tasks {
    pub count: String,
    pub items: Vec<TaskRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
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
    pub pending: bool,
    pub refused: Vec<String>,
}

/// What the pane holds while one session is shown.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Pane {
    draft: String,
    pending_message: Option<String>,
    refused_messages: Vec<String>,
    answers: BTreeMap<RequestId, Draft>,
    diffs: BTreeMap<HostSeq, DiffState>,
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
        at: At,
    ) -> (Self, Vec<Effect>) {
        match intent {
            SessionIntent::TypeMessage(draft) => {
                (Self { draft, ..self }, Vec::new())
            }
            action @ (SessionIntent::SendMessage
            | SessionIntent::RetryMessage) => self.send_message(
                id,
                session,
                at,
                action == SessionIntent::RetryMessage,
            ),
            SessionIntent::Interrupt => (
                self,
                Vec::from([notify(CommandBody::Interrupt { session: id }, at)]),
            ),
            SessionIntent::Continue => (
                Self {
                    focus_composer: true,
                    ..self
                },
                Vec::from([notify(CommandBody::Continue { session: id }, at)]),
            ),
            SessionIntent::Remove => (
                self,
                Vec::from([notify(
                    CommandBody::RemoveSession { session: id },
                    at,
                )]),
            ),
            SessionIntent::Finish => (
                self,
                Vec::from([notify(
                    CommandBody::FinishSession { session: id },
                    at,
                )]),
            ),
            SessionIntent::PickEffort(word) => {
                (self, set_effort(&word, id, session.effort, at))
            }
            SessionIntent::Decide(request, decision) => {
                self.decide(id, request, decision, at)
            }
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
                let effects = self.send_answer(request, id, session, at);
                (self, effects)
            }
            SessionIntent::ShowChanges(checkpoint) => {
                self.load_diff(id, checkpoint, at)
            }
            SessionIntent::CloseChanges(checkpoint) => {
                self.close_diff(checkpoint)
            }
            SessionIntent::Mounted(mount) => self.mounted(mount),
        }
    }

    fn decide(
        self,
        id: SessionId,
        request: RequestId,
        decision: Decision,
        at: At,
    ) -> (Self, Vec<Effect>) {
        (
            self,
            Vec::from([
                notify(
                    CommandBody::Resolve {
                        session: id,
                        request,
                        decision,
                    },
                    at,
                ),
                focus_card(request),
            ]),
        )
    }

    /// Sends the draft, once there is one and the agent takes messages.
    fn send_message(
        mut self,
        id: SessionId,
        session: &SessionView,
        at: At,
        retry: bool,
    ) -> (Self, Vec<Effect>) {
        if session.controls().composer != host_view::Composer::Open
            || self.pending_message.is_some()
        {
            return (self, Vec::new());
        }
        let draft = if retry {
            let Some(draft) = self.refused_messages.pop() else {
                return (self, Vec::new());
            };
            draft
        } else {
            if self.draft.trim().is_empty() {
                return (self, Vec::new());
            }
            core::mem::take(&mut self.draft)
        };
        let body = CommandBody::SendMessage {
            session: id,
            text: draft.trim().to_owned(),
        };
        self.pending_message = Some(draft);
        (self, Vec::from([at.send(body, ReplyTo::Message)]))
    }

    pub fn message_answered(&mut self, accepted: bool) {
        let Some(draft) = self.pending_message.take() else {
            return;
        };
        if !accepted {
            if self.draft.is_empty() {
                self.draft = draft;
            } else {
                self.refused_messages.push(draft);
            }
        }
    }

    /// Sends the answers once every question has one.
    fn send_answer(
        &self,
        request: RequestId,
        id: SessionId,
        session: &SessionView,
        at: At,
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
                        at,
                    ),
                    focus_card(request),
                ])
            })
    }

    fn close_diff(self, checkpoint: HostSeq) -> (Self, Vec<Effect>) {
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
        checkpoint: HostSeq,
        at: At,
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
            Vec::from([at.send(
                CommandBody::GetDiff {
                    session,
                    checkpoint,
                },
                ReplyTo::Diff {
                    checkpoint,
                    request,
                },
            )]),
        )
    }

    /// A diff's answer, unless a later request or a close replaced it.
    pub fn diffed(
        self,
        checkpoint: HostSeq,
        request: u64,
        answer: Result<CommandResult, String>,
        host: &str,
    ) -> Self {
        if self.diffs.get(&checkpoint) != Some(&DiffState::Loading(request)) {
            return self;
        }
        let mut diffs = self.diffs;
        diffs.insert(checkpoint, DiffState::answered(answer, host));
        Self { diffs, ..self }
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
                    "This session has no workspace snapshots.",
                ),
                |workspace| WorkspaceContext::Branch {
                    branch: format!("Branch: {}", workspace.branch),
                    directory: workspace.directory.0.clone(),
                },
            ),
            tasks: tasks(&session.tasks),
            empty: session
                .empty_transcript()
                .map(|kind| empty(kind, &provider)),
            entries: self.entries(session),
            other_agents: session
                .subagents
                .iter()
                .filter(|agent| {
                    !session
                        .items
                        .iter()
                        .filter_map(Item::call)
                        .any(|call| call == &agent.call)
                })
                .map(|agent| transcript::agent_group(agent, &self.answers))
                .collect(),
            working: session.working() == Working::Shown,
            notice,
            footer: self.footer(session, controls, &provider),
            status,
        }
    }

    fn footer(
        &self,
        session: &SessionView,
        controls: host_view::Controls,
        provider: &str,
    ) -> Footer {
        session.closed_reason().map_or_else(
            || {
                Footer::Composer(Composer {
                    text: self.draft.clone(),
                    pending: self.pending_message.is_some(),
                    refused: self.refused_messages.clone(),
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
                })
            },
            |reason| Footer::Closed {
                text: closed(reason),
                resume: controls.resume == Resume::Offered,
            },
        )
    }

    fn entries(&self, session: &SessionView) -> Vec<Entry> {
        let mut checkpoints = BTreeMap::new();
        for turn in &session.checkpoints {
            checkpoints.entry(turn.end).or_insert(turn);
        }
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
                changes: checkpoints.get(&index).map(|turn| {
                    checkpoints::changes(turn, self.diffs.get(&turn.checkpoint))
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

pub fn tasks(tasks: &[ChecklistEntry]) -> Option<Tasks> {
    if tasks.is_empty() {
        return None;
    }
    let done = tasks
        .iter()
        .filter(|task| task.status == ChecklistStatus::Completed)
        .count();
    Some(Tasks {
        count: format!("{done} of {} done", tasks.len()),
        items: tasks
            .iter()
            .map(|task| TaskRow {
                status: match task.status {
                    ChecklistStatus::Pending => "pending",
                    ChecklistStatus::InProgress => "in-progress",
                    ChecklistStatus::Completed => "completed",
                },
                label: match task.status {
                    ChecklistStatus::Pending => "To do",
                    ChecklistStatus::InProgress => "In progress",
                    ChecklistStatus::Completed => "Done",
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
    session.request_items().find_map(|item| match item {
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
pub struct Draft {
    chosen: Vec<Vec<String>>,
    typed: Vec<String>,
}

impl Draft {
    pub(crate) fn new(questions: &[Question]) -> Self {
        Self {
            chosen: questions.iter().map(|_| Vec::new()).collect(),
            typed: questions.iter().map(|_| String::new()).collect(),
        }
    }

    /// The labels chosen for `question`.
    #[must_use]
    pub(crate) fn chosen(&self, question: usize) -> &[String] {
        self.chosen.get(question).map_or(&[], Vec::as_slice)
    }

    #[must_use]
    pub(crate) fn typed(&self, question: usize) -> String {
        self.typed.get(question).cloned().unwrap_or_default()
    }

    /// Taps `label`. One choice replaces the last and clears the typed
    /// text; with many, a tap toggles the option.
    fn pick(
        self,
        question: usize,
        label: String,
        selection: Selection,
    ) -> Self {
        let Self {
            mut chosen,
            mut typed,
        } = self;
        if let Some(labels) = chosen.get_mut(question) {
            match (selection, labels.iter().position(|seen| seen == &label)) {
                (Selection::One, _) => {
                    labels.clear();
                    labels.push(label);
                }
                (Selection::Many, Some(at)) => {
                    labels.remove(at);
                }
                (Selection::Many, None) => labels.push(label),
            }
        }
        if let (Selection::One, Some(text)) =
            (selection, typed.get_mut(question))
        {
            text.clear();
        }
        Self { chosen, typed }
    }

    /// Types `text` as the answer. With one choice it replaces the option
    /// chosen; with many it is one more answer.
    fn r#type(self, question: usize, text: &str, selection: Selection) -> Self {
        let Self {
            mut chosen,
            mut typed,
        } = self;
        if let Some(answer) = typed.get_mut(question) {
            answer.clear();
            answer.push_str(text);
        }
        if let (Selection::One, Some(labels)) =
            (selection, chosen.get_mut(question))
        {
            labels.clear();
        }
        Self { chosen, typed }
    }

    /// Every question's answer, once every question has one.
    pub(crate) fn answers(&self) -> Option<Vec<Answer>> {
        let answers = self
            .chosen
            .iter()
            .zip(&self.typed)
            .map(|(chosen, typed)| {
                Answer(
                    chosen
                        .iter()
                        .cloned()
                        .chain(
                            Some(typed.trim())
                                .filter(|trimmed| !trimmed.is_empty())
                                .map(String::from),
                        )
                        .collect(),
                )
            })
            .collect::<Vec<Answer>>();
        Some(answers)
            .filter(|all| all.iter().all(|answer| !answer.0.is_empty()))
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
        let fresh =
            Draft::new(&[question(Selection::One), question(Selection::Many)]);
        let empty = fresh.answers();
        let first_only = fresh.pick(0, String::from("Red"), Selection::One);
        let one = first_only.answers();
        let complete = first_only
            .pick(0, String::from("Blue"), Selection::One)
            .pick(1, String::from("Apple"), Selection::Many)
            .pick(1, String::from("Pear"), Selection::Many)
            .pick(1, String::from("Apple"), Selection::Many)
            .r#type(1, " Plum ", Selection::Many);
        let both = complete.answers();
        let retyped = complete.r#type(0, "Green", Selection::One);
        let typed = retyped.answers();
        let repicked = retyped.pick(0, String::from("Red"), Selection::One);
        assert_eq!(
            (
                empty,
                one,
                both,
                typed,
                repicked.answers(),
                repicked.typed(0)
            ),
            (
                None,
                None,
                Some(Vec::from([answer(&["Blue"]), answer(&["Pear", "Plum"])])),
                Some(Vec::from([
                    answer(&["Green"]),
                    answer(&["Pear", "Plum"])
                ])),
                Some(Vec::from([answer(&["Red"]), answer(&["Pear", "Plum"])])),
                String::new(),
            )
        );
    }
    #[test]
    fn the_typed_text_is_kept_per_question() {
        let draft =
            Draft::new(&[question(Selection::One), question(Selection::One)])
                .r#type(1, "Green", Selection::One);
        assert_eq!(
            [draft.typed(0), draft.typed(1), draft.typed(2)],
            [String::new(), String::from("Green"), String::new()]
        );
    }
}
