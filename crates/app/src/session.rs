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

use dioxus::prelude::*;
use proto::{Decision, RequestId};
use ui::{
    Ask, EffortControl, Footer, Intent, Keystroke, Mount, QuestionBody, Row,
    SessionIntent, ToolDetail, UndoNote, WorkspaceContext,
};

use crate::checkpoints::TurnChanges;
use crate::sessions::Options;
use crate::{PaneHeader, Shell};

#[component]
pub fn SessionPane(pane: ui::SessionPane) -> Element {
    let shell = use_context::<Shell>();
    rsx! {
        section { class: "pane session-pane", "data-status": "{pane.status}",
            PaneHeader { title: "{pane.title}", subtitle: "{pane.subtitle}",
                span { class: "pill", "data-status": "{pane.status}", "{pane.pill}" }
                span { class: "runs-with", "{pane.model}" }
                Effort { effort: pane.effort }
                if pane.interrupt {
                    button {
                        class: "interrupt",
                        onclick: move |_| shell.dispatch(Intent::Session(SessionIntent::Interrupt)),
                        "Interrupt"
                    }
                }
            }
            Workspace { context: pane.workspace }
            Undo { note: pane.undo }
            if let Some(tasks) = pane.tasks {
                Tasks { tasks }
            }
            div { class: "transcript",
                div { class: "items", role: "log",
                    if let Some(text) = pane.empty {
                        p { class: "transcript-empty", "{text}" }
                    }
                    for entry in pane.entries {
                        TranscriptRow { key: "{entry.key}", row: entry.row }
                        if let Some(changes) = entry.changes {
                            TurnChanges { key: "{changes.key}", changes }
                        }
                    }
                    if pane.working {
                        p { class: "working", role: "status", "Thinking…" }
                    }
                }
            }
            div { class: "notices", role: "alert",
                if let Some(text) = pane.notice {
                    button { class: "notice", onclick: move |_| shell.dispatch(Intent::DismissNotice), "{text}" }
                }
            }
            Ending { footer: pane.footer }
        }
    }
}

/// The session's effort, which changes from the next turn on.
#[component]
fn Effort(effort: EffortControl) -> Element {
    let shell = use_context::<Shell>();
    match effort {
        EffortControl::Hidden => rsx! {},
        EffortControl::Fixed(text) => {
            rsx! { span { class: "runs-with", "{text}" } }
        }
        EffortControl::Picker(choices) => rsx! {
            select {
                class: "effort-pick",
                name: "session-effort",
                "aria-label": "Effort from the next turn",
                onchange: move |event| shell.dispatch(Intent::Session(SessionIntent::PickEffort(event.value()))),
                Options { choices }
            }
        },
    }
}

#[component]
fn Workspace(context: WorkspaceContext) -> Element {
    match context {
        WorkspaceContext::Plain(text) => {
            rsx! { p { class: "workspace-context", "{text}" } }
        }
        WorkspaceContext::Branch { branch, directory } => rsx! {
            div { class: "workspace-context",
                span { class: "workspace-branch", "{branch}" }
                details { class: "workspace-directory",
                    summary { "Working directory" }
                    code { "{directory}" }
                }
            }
        },
    }
}

#[component]
fn Undo(note: UndoNote) -> Element {
    let shell = use_context::<Shell>();
    match note {
        UndoNote::Hidden => rsx! {},
        UndoNote::Undoing(text) => {
            rsx! { p { class: "undo-status", role: "status", "{text}" } }
        }
        UndoNote::Failed { text, checkpoint } => rsx! {
            div { class: "undo-status error", role: "alert",
                p { "{text}" }
                button {
                    class: "undo-turn",
                    onclick: move |_| shell.dispatch(Intent::Session(SessionIntent::Undo(checkpoint))),
                    "Retry undo"
                }
            }
        },
        UndoNote::Undone { key, text } => rsx! {
            p {
                key: "{key}",
                class: "undo-status",
                role: "status",
                tabindex: "-1",
                onmounted: move |_| shell.dispatch(Intent::Session(SessionIntent::Mounted(Mount::Undone))),
                "{text}"
            }
        },
    }
}

/// The agent's checklist: open by default, and collapsible so a phone
/// keeps room for the transcript.
#[component]
fn Tasks(tasks: ui::Tasks) -> Element {
    rsx! {
        details { class: "tasks", open: true,
            summary {
                span { class: "tasks-title", "Tasks" }
                span { class: "tasks-count", "{tasks.count}" }
            }
            ol {
                for task in tasks.items {
                    li { key: "{task.key}", class: "task", "data-status": task.status,
                        span { class: "task-mark", role: "img", "aria-label": task.label }
                        span { class: "task-text", "{task.text}" }
                    }
                }
            }
        }
    }
}

#[component]
fn TranscriptRow(row: Row) -> Element {
    match row {
        Row::User { text, by } => rsx! {
            div { class: "user",
                p { class: "bubble", "{text}" }
                p { class: "by", "{by}" }
            }
        },
        Row::Reply { class, text } => rsx! { p { class, "{text}" } },
        // The live region is a span, so the summary stays a disclosure.
        Row::Thinking(text) => rsx! {
            details { class: "reasoning", "data-state": "streaming",
                summary { span { role: "status", "Thinking…" } }
                p { class: "reasoning-text", "{text}" }
            }
        },
        Row::Thought(text) => rsx! {
            details { class: "reasoning", "data-state": "done",
                summary { "Thinking" }
                p { class: "reasoning-text", "{text}" }
            }
        },
        Row::Continued(text) => rsx! { p { class: "continued", "{text}" } },
        Row::Tool(line) => rsx! { ToolLine { line } },
        Row::Approval(card) => rsx! { ApprovalCard { card } },
        Row::Question(card) => rsx! { QuestionCard { card } },
        Row::TurnEnd { outcome, text } => {
            rsx! { p { class: "turn-end", "data-outcome": outcome, "{text}" } }
        }
        Row::ModelChanged(text) => {
            rsx! { p { class: "model-changed", "{text}" } }
        }
        Row::Error(text) => rsx! { p { class: "error", "{text}" } },
    }
}

#[component]
fn ApprovalCard(card: ui::ApprovalCard) -> Element {
    let shell = use_context::<Shell>();
    let request = card.request;
    let decide = move |decision| {
        shell.dispatch(Intent::Session(SessionIntent::Decide(
            request, decision,
        )));
    };
    rsx! {
        section { class: "request", "data-open": "{card.open}", id: "{card.id}", tabindex: "-1",
            match card.ask {
                Ask::Open { actions, for_session } => rsx! {
                    p { class: "request-title", "Allow this?" }
                    ToolLine { line: card.tool }
                    Reason { reason: card.reason }
                    div { class: "actions",
                        for action in actions {
                            button { class: action.class, onclick: move |_| decide(action.decision), "{action.label}" }
                        }
                    }
                    if let Some(allows) = for_session {
                        button { class: "for-session", onclick: move |_| decide(Decision::ApproveForSession),
                            span { class: "for-session-title", "Allow for this session" }
                            span { class: "for-session-allows", "Allows " code { "{allows}" } " until the session ends" }
                        }
                    }
                },
                Ask::Resolved(text) => rsx! {
                    ToolLine { line: card.tool }
                    Reason { reason: card.reason }
                    p { class: "resolved", role: "status", "{text}" }
                },
            }
        }
    }
}

/// Why the agent asks, in its own words.
#[component]
fn Reason(reason: Option<String>) -> Element {
    rsx! {
        if let Some(why) = reason {
            p { class: "request-reason", "{why}" }
        }
    }
}

#[component]
fn ToolLine(line: ui::ToolLine) -> Element {
    rsx! {
        div { class: "tool", "data-state": line.state, title: line.state,
            span { class: "tool-mark", role: "img", "aria-label": line.state }
            match line.detail {
                ToolDetail::Command(command) => rsx! { code { class: "tool-detail", "{command}" } },
                ToolDetail::Files(files) => rsx! {
                    span { class: "tool-files",
                        for file in files {
                            span { class: "tool-file",
                                span { class: "tool-name", "{file.verb}" }
                                code { class: "tool-detail", "{file.path}" }
                            }
                        }
                    }
                },
                ToolDetail::NoFiles => rsx! { span { class: "tool-name", "Change files" } },
                ToolDetail::Other { name, detail } => rsx! {
                    span { class: "tool-name", "{name}" }
                    code { class: "tool-detail", "{detail}" }
                },
            }
        }
    }
}

/// Questions the agent asked: the form while open, then the answers.
#[component]
fn QuestionCard(card: ui::QuestionCard) -> Element {
    rsx! {
        section { class: "question", "data-open": "{card.open}", id: "{card.id}", tabindex: "-1",
            match card.body {
                QuestionBody::Form(form) => rsx! { QuestionForm { form } },
                QuestionBody::Answered { questions, resolved } => rsx! {
                    for question in questions {
                        p { class: "question-text", "{question.text}" }
                        if let Some(given) = question.answer {
                            p { class: "question-answer", "{given}" }
                        }
                    }
                    p { class: "resolved", role: "status", "{resolved}" }
                },
            }
        }
    }
}

#[component]
fn QuestionForm(form: ui::QuestionForm) -> Element {
    let shell = use_context::<Shell>();
    let request = form.request;
    rsx! {
        p { class: "request-title", "A question for you" }
        for field in form.fields {
            QuestionField { key: "{field.index}", request, field }
        }
        button {
            class: "approve send-answer",
            disabled: form.send_disabled,
            onclick: move |_| shell.dispatch(Intent::Session(SessionIntent::SendAnswer(request))),
            "Send answer"
        }
    }
}

#[component]
fn QuestionField(request: RequestId, field: ui::QuestionField) -> Element {
    let shell = use_context::<Shell>();
    let question = field.index;
    rsx! {
        fieldset { class: "question-field",
            legend { class: "question-text",
                span { class: "question-header", "{field.header}" }
                "{field.text}"
            }
            p { class: "question-hint", "{field.hint}" }
            div { class: "options",
                for option in field.options {
                    OptionButton { key: "{option.label}", request, question, option }
                }
            }
            input {
                class: "other",
                r#type: "text",
                placeholder: "Other answer",
                "aria-label": "Other answer",
                autocomplete: "off",
                value: "{field.typed}",
                oninput: move |event| shell.dispatch(Intent::Session(SessionIntent::TypeAnswer { request, question, text: event.value() })),
            }
        }
    }
}

#[component]
fn OptionButton(
    request: RequestId,
    question: usize,
    option: ui::OptionButton,
) -> Element {
    let shell = use_context::<Shell>();
    let label = option.label.clone();
    rsx! {
        button {
            class: "option",
            r#type: "button",
            "aria-pressed": option.pressed,
            onclick: move |_| shell.dispatch(Intent::Session(SessionIntent::Choose { request, question, label: label.clone() })),
            span { class: "option-label", "{option.label}" }
            span { class: "option-description", "{option.description}" }
        }
    }
}

/// A closed session's note and its Continue, or the composer.
#[component]
fn Ending(footer: Footer) -> Element {
    let shell = use_context::<Shell>();
    match footer {
        Footer::Closed { text, resume } => rsx! {
            div { class: "closed-note",
                p { "{text}" }
                if resume {
                    button {
                        class: "continue",
                        onclick: move |_| shell.dispatch(Intent::Session(SessionIntent::Continue)),
                        "Continue"
                    }
                }
            }
        },
        Footer::Composer(composer) => rsx! { Composer { composer } },
    }
}

#[component]
fn Composer(composer: ui::Composer) -> Element {
    let shell = use_context::<Shell>();
    let session = move |intent| shell.dispatch(Intent::Session(intent));
    rsx! {
        form { class: "composer",
            onsubmit: move |event: FormEvent| {
                event.prevent_default();
                session(SessionIntent::SendMessage);
            },
            textarea {
                name: "prompt",
                disabled: composer.disabled,
                rows: "1",
                placeholder: "{composer.placeholder}",
                autocomplete: "off",
                enterkeyhint: "send",
                value: "{composer.text}",
                oninput: move |event| session(SessionIntent::TypeMessage(event.value())),
                onkeydown: move |event| {
                    let key = Keystroke {
                        enter: event.key() == Key::Enter,
                        shift: event.modifiers().shift(),
                        composing: event.is_composing(),
                    };
                    if ui::sends(key) {
                        event.prevent_default();
                        session(SessionIntent::SendMessage);
                    }
                },
                onmounted: move |_| session(SessionIntent::Mounted(Mount::Composer)),
            }
            button {
                r#type: "submit",
                class: "send",
                "aria-label": "Send",
                disabled: composer.disabled,
            }
        }
    }
}
