//! Four sessions with no host behind them, folded through the real
//! `HostView`, so the screens can be seen and photographed in every state.
//! The host refuses every new session: Codex as unavailable, Claude as a
//! missing directory.

use client::host_view::HostView;
use client::{LinkState, PathKind, Status};
use dioxus::prelude::*;
use proto::{
    ClientName, CloseReason, CommandBody, CommandResult, Decision, Event, EventKind, HostPath,
    HostSeq, Provider, Rejection, RequestId, Resolution, SessionId, ToolCallId, ToolKind,
    ToolOutcome, ToolPhase, TurnOutcome,
};

use crate::{Link, Shell};

#[component]
pub(crate) fn Fixture() -> Element {
    let status = use_signal(|| {
        Some(Status {
            state: LinkState::Live,
            path: PathKind::Direct,
            host: Some(String::from("jim-desktop")),
            error: None,
            upgrade_error: None,
        })
    });
    let view = use_signal(view);
    let link = use_signal(|| Link::Fixture);
    rsx! { Shell { status, view, link, on_change_host: move |()| {} } }
}

pub(crate) fn answer(body: &CommandBody) -> CommandResult {
    match body {
        CommandBody::StartSession {
            provider: Provider::Codex,
            ..
        } => CommandResult::Rejected(Rejection::ProviderUnavailable {
            reason: text(
                "`codex app-server` did not start: No such file or directory (os error 2)",
            ),
        }),
        CommandBody::StartSession {
            provider: Provider::Claude,
            ..
        } => CommandResult::Rejected(Rejection::NoSuchDirectory),
        CommandBody::SendMessage { .. }
        | CommandBody::Interrupt { .. }
        | CommandBody::Resolve { .. } => CommandResult::Accepted,
    }
}

fn view() -> HostView {
    Vec::from([
        (4, created("/tmp/scratch")),
        (4, EventKind::SessionReady),
        (4, user("Run the flaky test 50 times and tell me how often it fails.", "desktop")),
        (4, ask(1, "Bash", "for i in $(seq 50); do cargo test -q flaky || echo FAIL; done")),
        (4, EventKind::RequestResolved { request: RequestId(1), resolution: Resolution::Withdrawn }),
        (4, EventKind::TurnCompleted { outcome: TurnOutcome::Interrupted }),
        (4, EventKind::SessionClosed { reason: CloseReason::Exited { status: text("exit status: 0") } }),
        (3, created("/home/jim/notes")),
        (3, EventKind::SessionReady),
        (3, user("Summarise this week's journal into weekly/2026-W45.md.", "desktop")),
        (3, edit("n1", &["weekly/2026-W45.md"])),
        (3, finished("n1", ToolOutcome::Succeeded)),
        (3, said("Done. I read the five daily notes and wrote weekly/2026-W45.md with three sections: what shipped, what is blocked, and what to pick up on Monday.")),
        (3, EventKind::TurnCompleted { outcome: TurnOutcome::Completed }),
        (2, created("/home/jim/src/website")),
        (2, EventKind::SessionReady),
        (2, user("Bump Astro to 5 and check that the build still passes.", "phone")),
        (2, said("Updating the dependency first, then I'll build.")),
        (2, edit("w1", &["package.json", "package-lock.json"])),
        (2, finished("w1", ToolOutcome::Succeeded)),
        (2, run("w2", "npm install")),
        (2, finished("w2", ToolOutcome::Succeeded)),
        (2, ask(2, "Bash", "npm run build && npx wrangler pages deploy dist --branch preview")),
        (1, created("/home/jim/src/conductor")),
        (1, EventKind::SessionReady),
        (1, user("The dump subcommand prints Debug output. Give it a --json flag and a test.", "phone")),
        (1, said("I'll start with how dump prints today.")),
        (1, run("c1", "rg -n \"fn dump\" crates/daemon/src")),
        (1, finished("c1", ToolOutcome::Succeeded)),
        (1, ask(3, "Edit", "crates/daemon/src/main.rs")),
        (1, EventKind::RequestResolved {
            request: RequestId(3),
            resolution: Resolution::Decided { decision: Decision::Approve, by: ClientName(text("phone")) },
        }),
        (1, edit("c2", &["crates/daemon/src/main.rs", "crates/daemon/tests/dump.rs"])),
        (1, finished("c2", ToolOutcome::Succeeded)),
        (1, run("c3", "cargo test -p daemon dump")),
        (1, finished("c3", ToolOutcome::Failed)),
        (1, delta("The first run failed: the test expected a trailing newline. ")),
        (1, delta("I've changed `dump --json` to end every event with one, so it pipes ")),
        (1, delta("straight into `jq`. Running the tests ag")),
    ])
    .into_iter()
    .enumerate()
    .map(|(index, (session, kind))| Event {
        seq: HostSeq(u64::try_from(index).unwrap_or_default()),
        session: SessionId(session),
        kind,
    })
    .fold(HostView::empty(), HostView::apply)
}

fn text(text: &str) -> String {
    String::from(text)
}

fn created(directory: &str) -> EventKind {
    EventKind::SessionCreated {
        provider: Provider::Claude,
        directory: HostPath(text(directory)),
        by: ClientName(text("desktop")),
    }
}

fn user(message: &str, by: &str) -> EventKind {
    EventKind::UserMessage {
        text: text(message),
        by: ClientName(text(by)),
    }
}

fn said(message: &str) -> EventKind {
    EventKind::AssistantMessage {
        text: text(message),
    }
}

fn delta(message: &str) -> EventKind {
    EventKind::AssistantDelta {
        text: text(message),
    }
}

fn started(call: &str, tool: ToolKind) -> EventKind {
    EventKind::ToolCall {
        call: ToolCallId(text(call)),
        phase: ToolPhase::Started { tool },
    }
}

fn run(call: &str, command: &str) -> EventKind {
    started(
        call,
        ToolKind::Command {
            command: text(command),
        },
    )
}

fn edit(call: &str, paths: &[&str]) -> EventKind {
    started(
        call,
        ToolKind::FileChange {
            paths: paths.iter().map(|path| text(path)).collect(),
        },
    )
}

fn finished(call: &str, outcome: ToolOutcome) -> EventKind {
    EventKind::ToolCall {
        call: ToolCallId(text(call)),
        phase: ToolPhase::Finished { outcome },
    }
}

fn ask(request: u64, tool: &str, detail: &str) -> EventKind {
    EventKind::RequestOpened {
        request: RequestId(request),
        tool: text(tool),
        detail: text(detail),
    }
}
