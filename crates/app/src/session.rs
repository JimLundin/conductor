//! One session: its header, transcript and composer.

#![expect(
    clippy::disallowed_types,
    reason = "rsx! caches each template in a OnceLock static"
)]
#![expect(
    clippy::empty_enums,
    reason = "derive(Props) on each component's props struct declares empty error enums for missing and repeated props"
)]
#![expect(
    clippy::shadow_reuse,
    reason = "derive(Props) on each component's props struct rebinds every prop in its builder"
)]
#![expect(
    clippy::useless_format,
    reason = "rsx! expands each \"{text}\" node to format!"
)]

use client::answers::Draft;
use client::host_view::{
    self, Approval, Interrupt, Item, ReplyState, RequestState, Resume,
    SessionStatus, SessionView, ToolState, UndoState, Working,
};
use client::new_session::{efforts, label, models};
use dioxus::prelude::*;
use proto::{
    Answer, Change, CloseReason, CommandBody, CommandResult, Decision, Effort,
    Providers, Question, RequestId, Resolution, Selection, SessionId, Task,
    TaskStatus, ToolKind, TurnOutcome,
};

use crate::checkpoints::TurnChanges;
use crate::sessions::short;
use crate::{Link, PaneHeader, run_command, send};

#[component]
pub(crate) fn SessionPane(
    id: SessionId,
    session: SessionView,
    providers: Option<Providers>,
    link: Signal<Link>,
    notice: Signal<Option<String>>,
    on_back: EventHandler<()>,
) -> Element {
    let controls = session.controls();
    let working = session.working();
    // Continue takes the button away; the composer that replaces it takes
    // the focus once the agent is ready again.
    let mut continued = use_signal(bool::default);
    let provider = label(session.provider);
    let status = match session.undo {
        UndoState::Undoing(_) => String::from("undoing"),
        UndoState::Failed { .. } => String::from("recovery needed"),
        UndoState::Ready | UndoState::Undone => session.status.word(),
    };
    let model = session
        .model
        .as_ref()
        .map_or_else(|| String::from("default model"), |model| model.0.clone());
    let offered = efforts(
        &models(providers.as_ref().map(|known| known.of(session.provider))),
        session.model.as_ref(),
    );
    let checkpoints = session.checkpoints.clone();
    let undo = session.undo.clone();
    let empty = match (&session.status, session.items.first()) {
        (_, Some(_)) => None,
        (SessionStatus::Starting, None) => {
            Some(format!("Starting {provider}…"))
        }
        (SessionStatus::Idle | SessionStatus::Running, None) => {
            Some(format!("{provider} is ready. Send a message to begin."))
        }
        (SessionStatus::Closed(_), None) => {
            Some(String::from("This session ended before it began."))
        }
    };
    rsx! {
        section { class: "pane session-pane", "data-status": "{status}",
            PaneHeader { title: "{short(&session.directory)}", subtitle: "{session.directory.0}", on_back,
                span { class: "pill", "data-status": "{status}", "{provider}, {status}" }
                span { class: "runs-with", "{model}" }
                EffortPicker { id, effort: session.effort, efforts: offered, link, notice }
                match controls.interrupt {
                    Interrupt::Offered => rsx! {
                        button {
                            class: "interrupt",
                            onclick: move |_| run_command(link, notice, CommandBody::Interrupt { session: id }),
                            "Interrupt"
                        }
                    },
                    Interrupt::Hidden => rsx! {},
                }
            }
            {session.workspace.as_ref().map_or_else(
                || rsx! { p { class: "workspace-context", "This session has no worktree checkpoints." } },
                |workspace| rsx! {
                    div { class: "workspace-context",
                        span { class: "workspace-branch", "Branch: {workspace.branch}" }
                        details { class: "workspace-directory",
                            summary { "Working directory" }
                            code { "{workspace.directory.0}" }
                        }
                    }
                },
            )}
            match undo.clone() {
                UndoState::Undoing(_) => rsx! { p { class: "undo-status", role: "status", "Undoing this turn and all later file and conversation changes…" } },
                UndoState::Failed { checkpoint, reason } => rsx! {
                    div { class: "undo-status error", role: "alert",
                        p { "Undo needs recovery: {reason}. Other actions are blocked until recovery finishes." }
                        button { class: "undo-turn", onclick: move |_| run_command(link, notice, CommandBody::UndoTurn { session: id, checkpoint }), "Retry undo" }
                    }
                },
                UndoState::Undone => rsx! { p { key: "undo-done-{id.0}", class: "undo-status", role: "status", tabindex: "-1", onmounted: move |_| { document::eval("requestAnimationFrame(() => (document.querySelector('.composer textarea:not(:disabled)') || document.querySelector('.undo-status'))?.focus())"); }, "Turn and all later file and conversation changes undone." } },
                UndoState::Ready => rsx! {},
            }
            Tasks { tasks: session.tasks.clone() }
            div { class: "transcript",
                div { class: "items", role: "log",
                    {empty.map_or_else(|| rsx! {}, |text| rsx! { p { class: "transcript-empty", "{text}" } })}
                    for (i, item) in session.items.into_iter().enumerate() {
                        TranscriptItem { key: "{i}", item,
                            on_decide: move |(request, decision)| {
                                run_command(link, notice, CommandBody::Resolve { session: id, request, decision });
                            },
                            on_answer: move |(request, answers)| {
                                run_command(link, notice, CommandBody::Answer { session: id, request, answers });
                            },
                        }
                        if let Some(checkpoint) = checkpoints.iter().find(|turn| turn.end == i).cloned() {
                            TurnChanges { key: "checkpoint-{checkpoint.checkpoint.0}", session: id, checkpoint: checkpoint.checkpoint, availability: checkpoint.undo, composer: controls.composer, undo: undo.clone(), link, notice }
                        }
                    }
                    match working {
                        Working::Shown => rsx! { p { class: "working", role: "status", "Thinking…" } },
                        Working::Hidden => rsx! {},
                    }
                }
            }
            div { class: "notices", role: "alert",
                {notice().map_or_else(|| rsx! {}, |text| rsx! {
                    button { class: "notice", onclick: move |_| notice.set(None), "{text}" }
                })}
            }
            match session.status {
                SessionStatus::Closed(reason) => rsx! {
                    div { class: "closed-note",
                        p { "{closed(&reason)}" }
                        match controls.resume {
                            Resume::Offered => rsx! {
                                button {
                                    class: "continue",
                                    onclick: move |_| {
                                        continued.set(true);
                                        run_command(link, notice, CommandBody::Continue { session: id });
                                    },
                                    "Continue"
                                }
                            },
                            Resume::Hidden => rsx! {},
                        }
                    }
                },
                SessionStatus::Starting | SessionStatus::Idle | SessionStatus::Running => rsx! {
                    Composer {
                        composer: controls.composer,
                        provider: provider,
                        focus: continued,
                        on_send: move |text| run_command(link, notice, CommandBody::SendMessage { session: id, text }),
                    }
                },
            }
        }
    }
}

/// The session's effort, which changes from the next turn on: a picker of
/// the model's efforts, or the effort alone when the host lists none.
#[component]
fn EffortPicker(
    id: SessionId,
    effort: Option<Effort>,
    efforts: Vec<Effort>,
    link: Signal<Link>,
    notice: Signal<Option<String>>,
) -> Element {
    match (efforts.first(), effort) {
        (None, Some(current)) => rsx! {
            span { class: "runs-with", "{current.wire()} effort" }
        },
        (None, None) => rsx! {},
        (Some(_), _) => rsx! {
            select {
                class: "effort-pick",
                name: "session-effort",
                "aria-label": "Effort from the next turn",
                onchange: move |event| {
                    if let Some(picked) = Effort::parse(&event.value()) {
                        set_effort(link, notice, (id, picked), effort);
                    }
                },
                match effort {
                    Some(_) => rsx! {},
                    None => rsx! { option { value: "", selected: true, disabled: true, "default effort" } },
                }
                for offered in efforts {
                    option {
                        value: "{offered.wire()}",
                        selected: Some(offered) == effort,
                        "{offered.wire()} effort"
                    }
                }
            }
        },
    }
}

/// Sends `SetEffort`. A refusal goes to the notice, and the picker goes
/// back to `shown`, the effort the session still has.
fn set_effort(
    link: Signal<Link>,
    mut notice: Signal<Option<String>>,
    picked: (SessionId, Effort),
    shown: Option<Effort>,
) {
    let (session, effort) = picked;
    spawn(async move {
        let refused = match send(
            link(),
            CommandBody::SetEffort { session, effort },
        )
        .await
        {
            Ok(CommandResult::Rejected(rejection)) => Some(rejection.label()),
            Ok(_) => None,
            Err(err) => Some(err),
        };
        if let Some(reason) = refused {
            notice.set(Some(reason));
            document::eval(&format!(
                "document.querySelector('select.effort-pick').value = '{}'",
                shown
                    .map_or_else(String::new, |kept| String::from(kept.wire()))
            ));
        }
    });
}

/// The agent's checklist, as the session holds it: open by default, and
/// collapsible so a phone keeps room for the transcript.
#[component]
fn Tasks(tasks: Vec<Task>) -> Element {
    let done = tasks
        .iter()
        .filter(|task| matches!(task.status, TaskStatus::Completed))
        .count();
    let total = tasks.len();
    match tasks.first() {
        None => rsx! {},
        Some(_) => rsx! {
            details { class: "tasks", open: true,
                summary {
                    span { class: "tasks-title", "Tasks" }
                    span { class: "tasks-count", "{done} of {total} done" }
                }
                ol {
                    for task in tasks {
                        li { key: "{task.id.0}", class: "task", "data-status": "{status_word(task.status)}",
                            span { class: "task-mark", role: "img", "aria-label": "{status_label(task.status)}" }
                            span { class: "task-text", "{task.text}" }
                        }
                    }
                }
            }
        },
    }
}

fn status_word(status: TaskStatus) -> String {
    String::from(match status {
        TaskStatus::Pending => "pending",
        TaskStatus::InProgress => "in-progress",
        TaskStatus::Completed => "completed",
    })
}

fn status_label(status: TaskStatus) -> String {
    String::from(match status {
        TaskStatus::Pending => "To do",
        TaskStatus::InProgress => "In progress",
        TaskStatus::Completed => "Done",
    })
}

fn closed(close: &CloseReason) -> String {
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

#[component]
fn TranscriptItem(
    item: Item,
    on_decide: EventHandler<(RequestId, Decision)>,
    on_answer: EventHandler<(RequestId, Vec<Answer>)>,
) -> Element {
    match item {
        Item::User { text, by } => rsx! {
            div { class: "user",
                p { class: "bubble", "{text}" }
                p { class: "by", "{by.0}" }
            }
        },
        Item::Reply { text, state } => rsx! {
            p { class: state.class(), "{text}" }
        },
        // Collapsed: the summary is the "Thinking…" line while it streams.
        // The live region is a span, so the summary stays a disclosure.
        Item::Reasoning {
            text,
            state: ReplyState::Streaming,
        } => rsx! {
            details { class: "reasoning", "data-state": "streaming",
                summary { span { role: "status", "Thinking…" } }
                p { class: "reasoning-text", "{text}" }
            }
        },
        Item::Reasoning {
            text,
            state: ReplyState::Done,
        } => rsx! {
            details { class: "reasoning", "data-state": "done",
                summary { "Thinking" }
                p { class: "reasoning-text", "{text}" }
            }
        },
        Item::Continued(reason) => rsx! {
            p { class: "continued", "Continued after: {closed(&reason)}" }
        },
        Item::Tool {
            tool,
            state,
            approval,
            ..
        } => rsx! { ToolItem { tool, state, approval, on_decide } },
        Item::Question {
            request,
            questions,
            state,
        } => rsx! { QuestionItem { request, questions, state, on_answer } },
        Item::TurnCompleted(outcome) => {
            let (attr, text) = match outcome {
                TurnOutcome::Completed => {
                    ("completed", String::from("Turn finished"))
                }
                TurnOutcome::Interrupted => {
                    ("interrupted", String::from("Interrupted"))
                }
                TurnOutcome::Failed { message } => {
                    ("failed", format!("Failed: {message}"))
                }
            };
            rsx! { p { class: "turn-end", "data-outcome": attr, "{text}" } }
        }
        Item::ModelChanged(reason) => {
            rsx! { p { class: "model-changed", "{reason}" } }
        }
        Item::Error(message) => rsx! { p { class: "error", "{message}" } },
    }
}

/// Sends the decision before focusing the card that remains after it resolves.
fn decide_and_focus(
    on_decide: EventHandler<(RequestId, Decision)>,
    request: RequestId,
    decision: Decision,
) {
    on_decide((request, decision));
    document::eval(&focus_on(request));
}

/// A request card's element id, unique in the pane, which shows one session.
fn card_id(request: RequestId) -> String {
    format!("request-{}", request.0)
}

/// Moves focus to a request's card, which stays when its buttons go.
fn focus_on(request: RequestId) -> String {
    format!("document.getElementById('{}').focus()", card_id(request))
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

/// A resolved question: each question with its answer, and who answered.
#[component]
fn Answered(questions: Vec<Question>, resolution: Resolution) -> Element {
    let answers = match &resolution {
        Resolution::Answered { answers, .. } => answers.clone(),
        Resolution::Decided { .. } | Resolution::Withdrawn => Vec::new(),
    };
    let answered = questions
        .into_iter()
        .zip(answers.into_iter().map(Some).chain(std::iter::repeat(None)))
        .map(|(question, answer)| {
            (question.text, answer.map(|given| given.0.join(", ")))
        })
        .collect::<Vec<(String, Option<String>)>>();
    rsx! {
        for (text, answer) in answered {
            p { class: "question-text", "{text}" }
            {answer.map_or_else(|| rsx! {}, |given| rsx! { p { class: "question-answer", "{given}" } })}
        }
        p { class: "resolved", role: "status", "{resolved(&resolution)}" }
    }
}

/// An open question: each one's options as buttons and a field for any
/// other answer, sent together once each question has an answer.
#[component]
fn QuestionForm(
    request: RequestId,
    questions: Vec<Question>,
    on_answer: EventHandler<(RequestId, Vec<Answer>)>,
) -> Element {
    let draft = use_signal(|| Draft::new(&questions));
    let complete = draft.read().answers();
    rsx! {
        p { class: "request-title", "A question for you" }
        for (index, question) in questions.into_iter().enumerate() {
            QuestionField { key: "{index}", index, question, draft }
        }
        button {
            class: "approve send-answer",
            disabled: complete.is_none(),
            onclick: move |_| {
                if let Some(answers) = draft.read().answers() {
                    on_answer((request, answers));
                    document::eval(&focus_on(request));
                }
            },
            "Send answer"
        }
    }
}

#[component]
fn QuestionField(
    index: usize,
    question: Question,
    draft: Signal<Draft>,
) -> Element {
    let selection = question.selection;
    let hint = match selection {
        Selection::One => "Choose one, or write your own",
        Selection::Many => "Choose any, and add your own",
    };
    let chosen = draft.read().chosen(index);
    let typed = draft.read().typed(index);
    rsx! {
        fieldset { class: "question-field",
            legend { class: "question-text",
                span { class: "question-header", "{question.header}" }
                " {question.text}"
            }
            p { class: "question-hint", "{hint}" }
            div { class: "options",
                for option in question.options {
                    OptionButton {
                        key: "{option.label}",
                        chosen: chosen.clone(),
                        label: option.label.clone(),
                        description: option.description,
                        on_pick: move |label| draft.set(draft().pick(index, label, selection)),
                    }
                }
            }
            input {
                class: "other",
                r#type: "text",
                placeholder: "Other answer",
                "aria-label": "Other answer",
                autocomplete: "off",
                value: "{typed}",
                oninput: move |event| draft.set(draft().r#type(index, &event.value(), selection)),
            }
        }
    }
}

#[component]
fn OptionButton(
    chosen: Vec<String>,
    label: String,
    description: String,
    on_pick: EventHandler<String>,
) -> Element {
    rsx! {
        button {
            class: "option",
            r#type: "button",
            "aria-pressed": chosen.iter().any(|seen| seen == &label),
            onclick: move |_| on_pick(label.clone()),
            span { class: "option-label", "{label}" }
            span { class: "option-description", "{description}" }
        }
    }
}

/// A tool call's outcome and what it acts on.
/// A tool call, and the approval card when it asked for one.
#[component]
fn ToolItem(
    tool: ToolKind,
    state: ToolState,
    approval: Approval,
    on_decide: EventHandler<(RequestId, Decision)>,
) -> Element {
    let state = state.label();
    match approval {
        Approval::NotAsked => rsx! { ToolLine { tool, state } },
        // One section for the open and the resolved card, so the
        // focus a button moves to it stays when the buttons go.
        Approval::Asked {
            request,
            reason,
            for_session,
            state: request_state,
        } => {
            let open = request_state.is_open();
            rsx! {
                section { class: "request", "data-open": "{open}", id: "{card_id(request)}", tabindex: "-1",
                    match request_state {
                        RequestState::Open => rsx! {
                            p { class: "request-title", "Allow this?" }
                            ToolLine { tool, state }
                            Reason { reason }
                            div { class: "actions",
                                button { class: "deny", onclick: move |_| decide_and_focus(on_decide, request, Decision::Deny), "Deny" }
                                button { class: "approve", onclick: move |_| decide_and_focus(on_decide, request, Decision::Approve), "Approve" }
                            }
                            {for_session.map_or_else(|| rsx! {}, |allows| rsx! {
                                button { class: "for-session", onclick: move |_| decide_and_focus(on_decide, request, Decision::ApproveForSession),
                                    span { class: "for-session-title", "Allow for this session" }
                                    span { class: "for-session-allows", "Allows " code { "{allows}" } " until the session ends" }
                                }
                            })}
                        },
                        RequestState::Resolved(resolution) => rsx! {
                            ToolLine { tool, state }
                            Reason { reason }
                            p { class: "resolved", role: "status", "{resolved(&resolution)}" }
                        },
                    }
                }
            }
        }
    }
}

/// Questions the agent asked: the form while open, then the answers.
#[component]
fn QuestionItem(
    request: RequestId,
    questions: Vec<Question>,
    state: RequestState,
    on_answer: EventHandler<(RequestId, Vec<Answer>)>,
) -> Element {
    let open = state.is_open();
    rsx! {
        section { class: "question", "data-open": "{open}", id: "{card_id(request)}", tabindex: "-1",
            match state {
                RequestState::Open => rsx! { QuestionForm { request, questions, on_answer } },
                RequestState::Resolved(resolution) => rsx! { Answered { questions, resolution } },
            }
        }
    }
}

#[component]
fn ToolLine(tool: ToolKind, state: String) -> Element {
    rsx! {
        div { class: "tool", "data-state": "{state}", title: "{state}",
            span { class: "tool-mark", role: "img", "aria-label": "{state}" }
            match tool {
                ToolKind::Command { command } => rsx! {
                    code { class: "tool-detail", "{command}" }
                },
                ToolKind::FileChange { files } => match files.first() {
                    Some(_) => rsx! {
                        span { class: "tool-files",
                            for file in files {
                                span { class: "tool-file",
                                    span { class: "tool-name", "{verb(file.change)}" }
                                    code { class: "tool-detail", "{file.path}" }
                                }
                            }
                        }
                    },
                    None => rsx! { span { class: "tool-name", "Change files" } },
                },
                ToolKind::Other { name, detail } => rsx! {
                    span { class: "tool-name", "{name}" }
                    code { class: "tool-detail", "{detail}" }
                },
            }
        }
    }
}

fn verb(change: Change) -> String {
    String::from(match change {
        Change::Write => "Write",
        Change::Edit => "Edit",
        Change::Delete => "Delete",
    })
}

/// Why the agent asks, in its own words.
#[component]
fn Reason(reason: Option<String>) -> Element {
    reason.map_or_else(
        || rsx! {},
        |why| rsx! { p { class: "request-reason", "{why}" } },
    )
}

#[component]
fn Composer(
    composer: host_view::Composer,
    provider: String,
    focus: Signal<bool>,
    on_send: EventHandler<String>,
) -> Element {
    let mut draft = use_signal(String::new);
    let placeholder = match composer {
        host_view::Composer::Open => format!("Message {provider}"),
        host_view::Composer::Busy => format!("{provider} is working…"),
        host_view::Composer::Closed => String::from("This session has ended"),
    };
    rsx! {
        form { class: "composer",
            onsubmit: move |event: FormEvent| {
                event.prevent_default();
                submit(draft, composer, on_send);
            },
            textarea {
                name: "prompt",
                disabled: !matches!(composer, host_view::Composer::Open),
                rows: "1",
                placeholder: "{placeholder}",
                autocomplete: "off",
                enterkeyhint: "send",
                value: "{draft}",
                oninput: move |event| draft.set(event.value()),
                onkeydown: move |event| send_on_enter(&event, draft, composer, on_send),
                onmounted: move |event| take_focus(event, focus),
            }
            button {
                r#type: "submit",
                class: "send",
                "aria-label": "Send",
                disabled: !matches!(composer, host_view::Composer::Open),
            }
        }
    }
}

fn take_focus(event: MountedEvent, mut focus: Signal<bool>) {
    if focus() {
        focus.set(false);
        spawn(async move {
            let _ = event.set_focus(true).await;
        });
    }
}

/// Return sends and Shift-Return starts a new line, except while an input
/// method is composing a character.
fn send_on_enter(
    event: &KeyboardEvent,
    draft: Signal<String>,
    composer: host_view::Composer,
    on_send: EventHandler<String>,
) {
    if event.key() == Key::Enter
        && !event.modifiers().shift()
        && !event.is_composing()
    {
        event.prevent_default();
        submit(draft, composer, on_send);
    }
}

fn submit(
    mut draft: Signal<String>,
    composer: host_view::Composer,
    on_send: EventHandler<String>,
) {
    match (composer, draft().trim()) {
        (host_view::Composer::Open, "")
        | (host_view::Composer::Busy | host_view::Composer::Closed, _) => {}
        (host_view::Composer::Open, text) => {
            on_send(text.to_string());
            draft.set(String::new());
        }
    }
}
