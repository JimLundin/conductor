//! A session's pane: what an empty transcript says, the agent's tasks, the
//! changes under the turn they end, and the cards and lines it draws.

use alloc::rc::Rc;

use proto::{
    Change, CheckpointId, CommandBody, CommandResult, EventKind, FileChange,
    RequestId, Task, TaskId, TaskStatus, ToolCallId, ToolKind, ToolPhase,
    UndoAvailability,
};

use super::{
    HOST, SESSION, a_turn, answer, changes, created, diff, event, opened,
    paired, pane, questions, rows, run, said, sent, text,
};
use crate::{
    DiffPanel, Effect, Focus, Intent, Place, Row, SessionIntent, TaskRow,
    Tasks, ToolDetail, ToolFile,
};

#[test]
fn an_empty_transcript_says_whether_the_agent_is_starting_or_ready() {
    let (starting, _) = run(
        paired(),
        Vec::from([
            event(1, created()),
            Intent::Show(Place::Session(HOST, SESSION)),
        ]),
    );
    let empty = |state: &crate::State| pane(state).map(|shown| shown.empty);
    assert_eq!(
        [
            empty(&starting),
            empty(&opened(Vec::new()).0),
            empty(&opened(Vec::from([event(3, said("hi"))])).0),
        ],
        [
            Some(Some(text("Starting Claude Code…"))),
            Some(Some(text("Claude Code is ready. Send a message to begin."))),
            Some(None),
        ]
    );
}

#[test]
fn the_agents_tasks_count_those_done() {
    let task = |id, words: &str, status| Task {
        id: TaskId(id),
        text: text(words),
        status,
        holder: SESSION,
    };
    let (state, _) = opened(Vec::from([event(
        3,
        EventKind::TasksChanged {
            tasks: Vec::from([
                task(1, "read", TaskStatus::Completed),
                task(2, "write", TaskStatus::InProgress),
                task(3, "test", TaskStatus::Pending),
            ]),
        },
    )]));
    let row = |key: &str, status, label, words: &str| TaskRow {
        key: text(key),
        status,
        label,
        text: text(words),
    };
    assert_eq!(
        (
            pane(&state).map(|shown| shown.tasks),
            pane(&opened(Vec::new()).0).map(|shown| shown.tasks),
        ),
        (
            Some(Some(Tasks {
                count: text("1 of 3 done"),
                items: Vec::from([
                    row("1", "completed", "Done", "read"),
                    row("2", "in-progress", "In progress", "write"),
                    row("3", "pending", "To do", "test"),
                ]),
            })),
            Some(None),
        )
    );
}

#[test]
fn a_turns_changes_hang_under_its_end_and_only_the_latest_load_shows() {
    let show = || Intent::Session(SessionIntent::ShowChanges(CheckpointId(1)));
    let (reopened, effects) = opened(
        a_turn(UndoAvailability::Available)
            .into_iter()
            .chain([
                show(),
                Intent::Session(SessionIntent::CloseChanges(CheckpointId(1))),
                show(),
            ])
            .collect(),
    );
    let (late, _) = run(
        reopened.clone(),
        Vec::from([answer(&effects, Ok(CommandResult::Diff(diff())))]),
    );
    assert_eq!(
        (
            pane(&reopened).map(|shown| shown
                .entries
                .into_iter()
                .map(|entry| entry.changes.is_some())
                .collect::<Vec<_>>()),
            changes(&late).map(|turn| turn.panel),
        ),
        (Some(Vec::from([false, true])), Some(DiffPanel::Loading))
    );
}

#[test]
fn an_answer_goes_to_the_question_it_was_given_for() {
    let (_, effects) = opened(Vec::from([
        event(3, said("go")),
        event(
            4,
            EventKind::QuestionAsked {
                request: RequestId(4),
                questions: questions(),
            },
        ),
        event(
            5,
            EventKind::QuestionAsked {
                request: RequestId(5),
                questions: questions().into_iter().take(1).collect(),
            },
        ),
        Intent::Session(SessionIntent::Choose {
            request: RequestId(5),
            question: 0,
            label: text("Red"),
        }),
        Intent::Session(SessionIntent::SendAnswer(RequestId(5))),
    ]));
    assert_eq!(
        sent(&effects),
        [CommandBody::Answer {
            session: SESSION,
            request: RequestId(5),
            answers: Vec::from([proto::Answer(Vec::from([text("Red")]))]),
        }]
    );
}

#[test]
fn a_file_change_lists_each_file_with_its_verb() {
    let file = |path: &str, change| FileChange {
        path: text(path),
        change,
    };
    let (state, _) = opened(Vec::from([
        event(3, said("go")),
        event(
            4,
            EventKind::ToolCall {
                call: ToolCallId(text("toolu_1")),
                phase: ToolPhase::Started {
                    tool: ToolKind::FileChange {
                        files: Vec::from([
                            file("a.rs", Change::Write),
                            file("b.rs", Change::Edit),
                            file("c.rs", Change::Delete),
                        ]),
                    },
                },
            },
        ),
    ]));
    let shown = |verb, path: &str| ToolFile {
        verb,
        path: text(path),
    };
    assert_eq!(
        rows(&state).pop().and_then(|row| match row {
            Row::Tool(line) => Some(line.detail),
            _ => None,
        }),
        Some(ToolDetail::Files(Vec::from([
            shown("Write", "a.rs"),
            shown("Edit", "b.rs"),
            shown("Delete", "c.rs"),
        ])))
    );
}

#[test]
fn a_diff_from_an_old_visit_does_not_answer_the_same_checkpoint_again() {
    let show = || Intent::Session(SessionIntent::ShowChanges(CheckpointId(1)));
    let (loading, old) = opened(
        a_turn(UndoAvailability::Available)
            .into_iter()
            .chain([show()])
            .collect(),
    );
    let (reopened, new) = run(
        loading,
        Vec::from([
            Intent::Show(Place::Sessions),
            Intent::Show(Place::Session(HOST, SESSION)),
            show(),
        ]),
    );
    let (still_loading, _) = run(
        reopened,
        Vec::from([answer(&old, Ok(CommandResult::Diff(diff())))]),
    );
    let (loaded, _) = run(
        still_loading.clone(),
        Vec::from([answer(&new, Ok(CommandResult::Diff(diff())))]),
    );
    assert_eq!(
        changes(&still_loading).map(|turn| turn.panel),
        Some(DiffPanel::Loading)
    );
    assert!(matches!(
        changes(&loaded).map(|turn| turn.panel),
        Some(DiffPanel::Loaded(_))
    ));
}

#[test]
fn a_host_reset_drops_the_old_sessions_draft_and_pending_diff() {
    let show = Intent::Session(SessionIntent::ShowChanges(CheckpointId(1)));
    let turn = a_turn(UndoAvailability::Available);
    let (loading, old) = opened(
        turn.iter()
            .cloned()
            .chain([
                Intent::Session(SessionIntent::TypeMessage(text("old draft"))),
                show.clone(),
            ])
            .collect(),
    );
    let (reset, _) = run(
        loading,
        Vec::from([Intent::Link(
            HOST,
            client::link::Update::Reset(proto::Epoch(2)),
        )]),
    );
    assert!(pane(&reset).is_none());
    let (reopened, _) = run(
        reset,
        [event(1, created()), event(2, EventKind::SessionReady)]
            .into_iter()
            .chain(turn)
            .chain([show, answer(&old, Ok(CommandResult::Diff(diff())))])
            .collect(),
    );
    assert_eq!(
        changes(&reopened).map(|turn| turn.panel),
        Some(DiffPanel::Loading)
    );
    assert_eq!(
        super::composer(&reopened).map(|composer| composer.0),
        Some(String::new())
    );
}

#[test]
fn failed_changes_can_close_with_focus_returned_or_retry() {
    let checkpoint = CheckpointId(1);
    let show = Intent::Session(SessionIntent::ShowChanges(checkpoint));
    let (loading, requested) = opened(
        a_turn(UndoAvailability::Available)
            .into_iter()
            .chain([show.clone()])
            .collect(),
    );
    let (failed, _) = run(
        loading,
        Vec::from([answer(&requested, Err(text("offline")))]),
    );
    assert_eq!(
        changes(&failed).map(|shown| (shown.panel, shown.button_disabled)),
        Some((DiffPanel::Failed(text("offline")), false))
    );
    let (_, retry) = run(failed.clone(), Vec::from([show]));
    assert_eq!(sent(&retry), sent(&requested));
    let (hidden, focus) = run(
        failed,
        Vec::from([Intent::Session(SessionIntent::CloseChanges(checkpoint))]),
    );
    assert_eq!(
        changes(&hidden).map(|shown| shown.panel),
        Some(DiffPanel::Hidden)
    );
    assert_eq!(
        focus,
        [Effect::Focus(Focus::NextFrame(text(
            "checkpoint-1-changes"
        )))]
    );
}

#[test]
fn a_transcript_rebuild_reuses_the_open_preview() {
    let checkpoint = CheckpointId(1);
    let (loading, requested) = opened(
        a_turn(UndoAvailability::Available)
            .into_iter()
            .chain([Intent::Session(SessionIntent::ShowChanges(checkpoint))])
            .collect(),
    );
    let mut large = diff();
    large.files = (0..4)
        .map(|index| proto::CheckpointFile {
            path: alloc::format!("{index}.txt"),
            change: Change::Edit,
            patch: proto::FilePatch::Text("a".repeat(64 * 1024)),
        })
        .collect();
    let (loaded, _) = run(
        loading,
        Vec::from([answer(&requested, Ok(CommandResult::Diff(large)))]),
    );
    let (busy, _) =
        run(loaded.clone(), Vec::from([event(6, said("next turn"))]));
    let before_panel = changes(&loaded).map(|shown| shown.panel);
    let after_panel = changes(&busy).map(|shown| shown.panel);
    assert!(matches!(
        (before_panel, after_panel),
        (Some(DiffPanel::Loaded(before)), Some(DiffPanel::Loaded(after)))
            if Rc::ptr_eq(&before.preview, &after.preview)
                && !before.undo_disabled
                && after.undo_disabled
    ));
}
