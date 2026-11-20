//! A session's pane: what an empty transcript says, the agent's tasks, the
//! changes under the turn they end, and the cards and lines it draws.

use proto::{
    Change, CheckpointId, CommandBody, CommandResult, EventKind, FileChange,
    RequestId, Task, TaskId, TaskStatus, ToolCallId, ToolKind, ToolPhase,
    UndoAvailability,
};

use super::{
    HOST, SESSION, a_turn, changes, created, diff, diff_reply, event, opened,
    paired, pane, questions, rows, run, said, sent, text,
};
use crate::{
    DiffPanel, Intent, Place, Row, SessionIntent, TaskRow, Tasks, ToolDetail,
    ToolFile,
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
    let (reopened, _) = opened(
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
        Vec::from([Intent::Replied(
            diff_reply(),
            Ok(CommandResult::Diff(diff())),
        )]),
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
