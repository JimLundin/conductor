use agents::{NativeRequest, NativeTurn, Reply, Step};
use proto::{
    Answer, Checkpoint, CheckpointId, ClientName, CloseReason, CommandResult,
    Decision, Effort, Event, EventKind, HistoryAnchor, HostPath, HostSeq,
    ModelChoice, ModelId, Models, Provider, Question, Rejection, RequestId,
    Resolution, Selection, SessionId, ToolCallId, ToolKind, ToolOutcome,
    ToolPhase, TurnOutcome, UndoAvailability, Workspace,
};

use super::{
    Asked, Call, Choice, ClaudeResume, Effect, Input, Launching, Order, Resume,
    Session, Unstarted, fits,
};

fn event(kind: EventKind) -> Event {
    Event {
        seq: HostSeq(1),
        session: SessionId(1),
        kind,
    }
}

fn text(value: &str) -> String {
    String::from(value)
}

fn phone() -> ClientName {
    ClientName(text("phone"))
}

const fn no_choice() -> Choice {
    Choice {
        model: None,
        effort: None,
    }
}

fn restored(history: &[Event], conversation: Option<&str>) -> Session {
    Session::restore(
        SessionId(1),
        HostPath(text("/work")),
        no_choice(),
        conversation.map(String::from),
        history,
    )
}

/// Steps `session` through each input, checking the effects of each.
fn expect(session: Session, script: Vec<(Input, Vec<Effect>)>) -> Session {
    script.into_iter().fold(session, |now, (input, expected)| {
        let shown = format!("{input:?}");
        let (next, effects) = now.step(input);
        assert_eq!(effects, expected, "after {shown}");
        next
    })
}

fn launching(directory: &str, resume: Resume) -> Effect {
    Effect::Call(Call::Start(Launching {
        directory: HostPath(text(directory)),
        choice: no_choice(),
        resume,
    }))
}

fn claude(resume: ClaudeResume) -> Effect {
    launching("/work", Resume::Claude(resume))
}

fn start() -> Input {
    Input::Order(Order::Start {
        provider: Provider::Claude,
        offered: Models::none(),
    })
}

fn send(message: &str) -> Input {
    Input::Order(Order::Send {
        text: text(message),
        by: phone(),
    })
}

fn record(message: &str) -> Effect {
    Effect::Call(Call::Record(EventKind::UserMessage {
        text: text(message),
        by: phone(),
    }))
}

fn sending(message: &str) -> Effect {
    Effect::Call(Call::Send {
        text: text(message),
        effort: None,
    })
}

fn turn_started() -> Input {
    Input::Step(Step::TurnStarted(NativeTurn(text("conv"))))
}

fn turn_ended() -> Input {
    Input::Step(Step::TurnEnded(TurnOutcome::Completed))
}

fn completed() -> Effect {
    Effect::Emit(EventKind::TurnCompleted {
        outcome: TurnOutcome::Completed,
    })
}

fn saved() -> Effect {
    Effect::Conversation(text("conv"))
}

fn ok() -> Input {
    Input::Done(Ok(()))
}

fn accepted() -> Effect {
    Effect::Reply(CommandResult::Accepted)
}

fn rejected(rejection: Rejection) -> Effect {
    Effect::Reply(CommandResult::Rejected(rejection))
}

fn ready() -> Session {
    expect(
        restored(&[], None),
        Vec::from([
            (start(), Vec::from([claude(ClaudeResume::Fresh)])),
            (
                Input::Started(Ok(text("conv"))),
                Vec::from([
                    saved(),
                    Effect::Emit(EventKind::SessionReady),
                    accepted(),
                ]),
            ),
        ]),
    )
}

/// A Claude session whose turn has begun.
fn running() -> Session {
    expect(
        ready(),
        Vec::from([
            (send("hi"), Vec::from([record("hi")])),
            (Input::Recorded(HostSeq(4)), Vec::from([sending("hi")])),
            (ok(), Vec::from([saved(), accepted()])),
            (turn_started(), Vec::from([saved()])),
        ]),
    )
}

fn open() -> Input {
    Input::Step(Step::Open {
        native: NativeRequest(text("perm-1")),
        call: ToolCallId(text("toolu_1")),
        tool: ToolKind::Command {
            command: text("ls"),
        },
        reason: None,
        for_session: None,
    })
}

fn reply(request: u64, reply: Reply, by: &str) -> Input {
    Input::Order(Order::Reply {
        request: RequestId(request),
        reply,
        by: ClientName(text(by)),
    })
}

#[test]
fn a_turn_runs_from_the_message_to_its_end() {
    let _ = expect(
        ready(),
        Vec::from([
            (send("hi"), Vec::from([record("hi")])),
            (Input::Recorded(HostSeq(4)), Vec::from([sending("hi")])),
            (ok(), Vec::from([saved(), accepted()])),
            (
                send("again"),
                Vec::from([rejected(Rejection::TurnInProgress)]),
            ),
            (turn_started(), Vec::from([saved()])),
            (turn_ended(), Vec::from([completed()])),
            (send("next"), Vec::from([record("next")])),
        ]),
    );
}

#[test]
fn a_message_the_agent_never_got_fails_the_turn() {
    let _ = expect(
        ready(),
        Vec::from([
            (send("hi"), Vec::from([record("hi")])),
            (Input::Recorded(HostSeq(4)), Vec::from([sending("hi")])),
            (
                Input::Done(Err(text("broken pipe"))),
                Vec::from([
                    Effect::Emit(EventKind::TurnCompleted {
                        outcome: TurnOutcome::Failed {
                            message: text("broken pipe"),
                        },
                    }),
                    accepted(),
                ]),
            ),
            (send("again"), Vec::from([record("again")])),
        ]),
    );
}

#[test]
fn a_request_is_opened_answered_once_and_withdrawn_when_the_turn_ends() {
    let deny = || Reply::Decide(Decision::Deny);
    let _ = expect(
        running(),
        Vec::from([
            (
                open(),
                Vec::from([Effect::Emit(EventKind::RequestOpened {
                    request: RequestId(1),
                    call: ToolCallId(text("toolu_1")),
                    tool: ToolKind::Command {
                        command: text("ls"),
                    },
                    reason: None,
                    for_session: None,
                })]),
            ),
            (
                reply(1, deny(), "phone"),
                Vec::from([
                    Effect::Emit(EventKind::RequestResolved {
                        request: RequestId(1),
                        resolution: Resolution::Decided {
                            decision: Decision::Deny,
                            by: phone(),
                        },
                    }),
                    Effect::Call(Call::Answer {
                        native: NativeRequest(text("perm-1")),
                        reply: deny(),
                        by: phone(),
                    }),
                ]),
            ),
            (ok(), Vec::from([accepted()])),
            (
                reply(1, deny(), "laptop"),
                Vec::from([rejected(Rejection::AlreadyResolved {
                    by: phone(),
                })]),
            ),
            (
                Input::Step(Step::ToolFinished {
                    call: ToolCallId(text("toolu_1")),
                    outcome: ToolOutcome::Failed,
                }),
                Vec::from([Effect::Emit(EventKind::ToolCall {
                    call: ToolCallId(text("toolu_1")),
                    phase: ToolPhase::Finished {
                        outcome: ToolOutcome::Declined,
                    },
                })]),
            ),
            (
                Input::Step(Step::Ask {
                    native: NativeRequest(text("ask-1")),
                    questions: Vec::new(),
                }),
                Vec::from([Effect::Emit(EventKind::QuestionAsked {
                    request: RequestId(2),
                    questions: Vec::new(),
                })]),
            ),
            (
                turn_ended(),
                Vec::from([
                    completed(),
                    Effect::Emit(EventKind::RequestResolved {
                        request: RequestId(2),
                        resolution: Resolution::Withdrawn,
                    }),
                ]),
            ),
            (
                reply(2, Reply::Answer(Vec::new()), "phone"),
                Vec::from([rejected(Rejection::UnknownRequest)]),
            ),
        ]),
    );
}

#[test]
fn a_reply_that_does_not_fit_the_request_is_rejected() {
    let questions = Vec::from([Question {
        header: text("Color"),
        text: text("Which?"),
        options: Vec::new(),
        selection: Selection::One,
    }]);
    let _ = expect(
        running(),
        Vec::from([
            (
                Input::Step(Step::Ask {
                    native: NativeRequest(text("ask-1")),
                    questions: questions.clone(),
                }),
                Vec::from([Effect::Emit(EventKind::QuestionAsked {
                    request: RequestId(1),
                    questions,
                })]),
            ),
            (
                reply(1, Reply::Decide(Decision::Approve), "phone"),
                Vec::from([rejected(Rejection::WrongReply {
                    reason: text("a question takes answers, not a decision"),
                })]),
            ),
            (
                reply(9, Reply::Answer(Vec::new()), "phone"),
                Vec::from([rejected(Rejection::UnknownRequest)]),
            ),
        ]),
    );
}

#[test]
fn an_interrupt_before_the_turn_starts_waits_for_it() {
    let interrupt = || Input::Order(Order::Interrupt);
    let interrupting =
        || Effect::Call(Call::Interrupt(NativeTurn(text("conv"))));
    let _ = expect(
        ready(),
        Vec::from([
            (interrupt(), Vec::from([rejected(Rejection::NoTurnRunning)])),
            (send("hi"), Vec::from([record("hi")])),
            (Input::Recorded(HostSeq(4)), Vec::from([sending("hi")])),
            (ok(), Vec::from([saved(), accepted()])),
            (
                interrupt(),
                Vec::from([
                    Effect::Trace(
                        "holding the interrupt until the turn starts",
                    ),
                    accepted(),
                ]),
            ),
            (
                turn_started(),
                Vec::from([
                    saved(),
                    Effect::Trace(
                        "the turn started: sending the held interrupt",
                    ),
                    interrupting(),
                ]),
            ),
            (
                Input::Done(Err(text("gone"))),
                Vec::from([Effect::Emit(EventKind::Error {
                    message: text("gone"),
                })]),
            ),
            (interrupt(), Vec::from([interrupting()])),
            (ok(), Vec::from([accepted()])),
        ]),
    );
}

#[test]
fn an_effort_the_model_takes_applies_from_the_next_turn() {
    let offered = Models {
        choices: Vec::from([ModelChoice {
            id: ModelId(text("opus")),
            name: text("Opus"),
            description: String::new(),
            efforts: Vec::from([Effort::Low, Effort::High]),
        }]),
        default: Some(ModelId(text("opus"))),
    };
    let set = |effort| Input::Order(Order::Effort(effort, offered.clone()));
    let _ = expect(
        ready(),
        Vec::from([
            (
                set(Effort::High),
                Vec::from([
                    Effect::Emit(EventKind::EffortChanged {
                        effort: Effort::High,
                    }),
                    accepted(),
                ]),
            ),
            (set(Effort::High), Vec::from([accepted()])),
            (
                set(Effort::Medium),
                Vec::from([rejected(Rejection::NotOffered {
                    reason: text("Opus doesn't take the effort medium"),
                })]),
            ),
            (send("hi"), Vec::from([record("hi")])),
            (
                Input::Recorded(HostSeq(4)),
                Vec::from([Effect::Call(Call::SetEffort(Effort::High))]),
            ),
            (ok(), Vec::from([sending("hi")])),
            (ok(), Vec::from([saved(), accepted()])),
            (turn_started(), Vec::from([saved()])),
            (turn_ended(), Vec::from([completed()])),
            (send("later"), Vec::from([record("later")])),
            (Input::Recorded(HostSeq(8)), Vec::from([sending("later")])),
        ]),
    );
}

#[test]
fn an_agent_that_exits_mid_turn_closes_the_session_until_it_starts_again() {
    let _ = expect(
        running(),
        Vec::from([
            (
                open(),
                Vec::from([Effect::Emit(EventKind::RequestOpened {
                    request: RequestId(1),
                    call: ToolCallId(text("toolu_1")),
                    tool: ToolKind::Command {
                        command: text("ls"),
                    },
                    reason: None,
                    for_session: None,
                })]),
            ),
            (
                Input::Exited,
                Vec::from([
                    Effect::Release,
                    Effect::Emit(EventKind::SessionClosed {
                        reason: CloseReason::Exited {
                            status: text("the agent's events ended"),
                        },
                    }),
                ]),
            ),
            (send("hi"), Vec::from([rejected(Rejection::SessionClosed)])),
            (
                start(),
                Vec::from([claude(ClaudeResume::Session(text("conv")))]),
            ),
            (
                Input::Started(Ok(text("conv"))),
                Vec::from([
                    saved(),
                    Effect::Emit(EventKind::SessionReady),
                    accepted(),
                ]),
            ),
            (
                reply(1, Reply::Decide(Decision::Approve), "phone"),
                Vec::from([rejected(Rejection::UnknownRequest)]),
            ),
        ]),
    );
}

#[test]
fn a_start_whose_conversation_is_missing_tries_the_next_resume() {
    let history = [
        event(EventKind::SessionCreated {
            provider: Provider::Claude,
            directory: HostPath(text("/work")),
            by: phone(),
            model: None,
            effort: None,
        }),
        event(EventKind::HistoryMaterialized {
            history: HistoryAnchor::Claude {
                conversation: text("conv"),
                message: None,
            },
        }),
    ];
    let missing = |session: &str| {
        Input::Started(Err(Unstarted {
            message: text("missing"),
            missing: Some(text(session)),
        }))
    };
    let _ = expect(
        restored(&history, Some("conv")),
        Vec::from([
            (
                start(),
                Vec::from([claude(ClaudeResume::Session(text("conv")))]),
            ),
            (missing("conv"), Vec::from([claude(ClaudeResume::Fresh)])),
            (
                missing("other"),
                Vec::from([
                    Effect::Emit(EventKind::SessionClosed {
                        reason: CloseReason::FailedToStart {
                            message: text("missing"),
                        },
                    }),
                    accepted(),
                ]),
            ),
        ]),
    );
}

#[test]
fn a_fork_whose_target_exists_resumes_the_target() {
    let history = [event(EventKind::UndoHistoryRestored {
        checkpoint: CheckpointId(2),
        history: HistoryAnchor::ClaudeFork {
            source: text("source"),
            through: text("boundary"),
            target: text("target"),
        },
    })];
    let _ = expect(
        restored(&history, None),
        Vec::from([
            (
                start(),
                Vec::from([claude(ClaudeResume::Session(text("target")))]),
            ),
            (
                Input::Started(Err(Unstarted {
                    message: text("missing"),
                    missing: Some(text("target")),
                })),
                Vec::from([claude(ClaudeResume::Fork {
                    source: text("source"),
                    through: text("boundary"),
                    target: text("target"),
                })]),
            ),
        ]),
    );
}

fn workspace() -> Workspace {
    Workspace {
        repository: HostPath(text("/repo")),
        original: HostPath(text("/repo")),
        worktree: HostPath(text("/worktree")),
        directory: HostPath(text("/work")),
        branch: text("conductor/1"),
    }
}

fn checkpoint(
    id: u64,
    history: HistoryAnchor,
    undo: UndoAvailability,
) -> Event {
    event(EventKind::CheckpointCreated {
        checkpoint: Checkpoint {
            id: CheckpointId(id),
            parent: id.checked_sub(1).map(CheckpointId),
            turn_start: Some(HostSeq(id)),
            commit: format!("c{id}"),
            history,
            undo,
        },
    })
}

fn undo(provider: Provider) -> Input {
    Input::Order(Order::Undo {
        checkpoint: CheckpointId(1),
        provider,
        fork: text("fork"),
    })
}

fn restore() -> Effect {
    Effect::Call(Call::Restore {
        workspace: workspace(),
        commit: text("c0"),
    })
}

fn preflight() -> Effect {
    Effect::Call(Call::Preflight {
        workspace: workspace(),
        commit: text("c0"),
    })
}

fn undone() -> Vec<Effect> {
    Vec::from([
        Effect::Emit(EventKind::TurnsUndone {
            checkpoint: CheckpointId(1),
            restored: CheckpointId(0),
        }),
        accepted(),
    ])
}

fn history_restored(history: HistoryAnchor) -> Effect {
    Effect::Emit(EventKind::UndoHistoryRestored {
        checkpoint: CheckpointId(1),
        history,
    })
}

#[test]
fn an_undo_restores_history_then_files_and_resumes_after_a_failure() {
    let history = [
        event(EventKind::WorkspaceCreated {
            workspace: workspace(),
        }),
        checkpoint(0, HistoryAnchor::Fresh, UndoAvailability::Available),
        checkpoint(
            1,
            HistoryAnchor::Claude {
                conversation: text("conv"),
                message: Some(text("m1")),
            },
            UndoAvailability::Available,
        ),
    ];
    let _ = expect(
        restored(&history, None),
        Vec::from([
            (undo(Provider::Claude), Vec::from([preflight()])),
            (
                ok(),
                Vec::from([
                    Effect::Emit(EventKind::UndoStarted {
                        checkpoint: CheckpointId(1),
                        restore: CheckpointId(0),
                        history: HistoryAnchor::Fresh,
                    }),
                    Effect::Barrier,
                    Effect::Call(Call::Exists(HostPath(text("/work")))),
                ]),
            ),
            (
                Input::Exists(true),
                Vec::from([Effect::Call(Call::Shutdown)]),
            ),
            (ok(), Vec::from([claude(ClaudeResume::Fresh)])),
            (
                Input::Started(Ok(text("new"))),
                Vec::from([
                    Effect::Conversation(text("new")),
                    history_restored(HistoryAnchor::Fresh),
                    Effect::Call(Call::History),
                ]),
            ),
            (
                Input::History(Vec::new()),
                Vec::from([Effect::Barrier, restore()]),
            ),
            (
                Input::Done(Err(text("dirty"))),
                Vec::from([
                    Effect::Emit(EventKind::UndoFailed {
                        checkpoint: CheckpointId(1),
                        reason: text("dirty"),
                    }),
                    rejected(Rejection::UndoFailed {
                        reason: text("dirty"),
                    }),
                ]),
            ),
            (send("hi"), Vec::from([rejected(Rejection::UndoInProgress)])),
            (undo(Provider::Claude), Vec::from([restore()])),
            (ok(), undone()),
            (
                undo(Provider::Claude),
                Vec::from([rejected(Rejection::CheckpointUnavailable)]),
            ),
        ]),
    );
}

#[test]
fn an_undo_in_a_gone_directory_restores_the_codex_thread_in_the_worktree() {
    let codex = |turn: &str| HistoryAnchor::Codex {
        conversation: text("thread"),
        turn: Some(text(turn)),
    };
    let history = [
        event(EventKind::WorkspaceCreated {
            workspace: workspace(),
        }),
        checkpoint(0, codex("t0"), UndoAvailability::Available),
        checkpoint(1, codex("t1"), UndoAvailability::Available),
    ];
    let turns = |listed: &[&str]| {
        Input::Turns(Ok(listed.iter().copied().map(String::from).collect()))
    };
    let _ = expect(
        restored(&history, None),
        Vec::from([
            (undo(Provider::Codex), Vec::from([preflight()])),
            (
                ok(),
                Vec::from([
                    Effect::Emit(EventKind::UndoStarted {
                        checkpoint: CheckpointId(1),
                        restore: CheckpointId(0),
                        history: codex("t1"),
                    }),
                    Effect::Barrier,
                    Effect::Call(Call::Exists(HostPath(text("/work")))),
                ]),
            ),
            (
                Input::Exists(false),
                Vec::from([launching(
                    "/worktree",
                    Resume::Codex(Some(text("thread"))),
                )]),
            ),
            (
                Input::Started(Ok(text("thread"))),
                Vec::from([Effect::Call(Call::Turns)]),
            ),
            (
                turns(&["t0", "t1"]),
                Vec::from([Effect::Call(Call::Revert(text("t1")))]),
            ),
            (ok(), Vec::from([Effect::Call(Call::Turns)])),
            (turns(&["t0"]), Vec::from([Effect::Call(Call::Shutdown)])),
            (
                ok(),
                Vec::from([
                    history_restored(codex("t0")),
                    Effect::Call(Call::History),
                ]),
            ),
            (
                Input::History(Vec::new()),
                Vec::from([Effect::Barrier, restore()]),
            ),
            (
                ok(),
                Vec::from([launching(
                    "/work",
                    Resume::Codex(Some(text("thread"))),
                )]),
            ),
            (Input::Started(Ok(text("thread"))), undone()),
        ]),
    );
}

#[test]
fn a_restored_session_resumes_the_fork_or_the_conversation_saved_after_it() {
    let created = event(EventKind::SessionCreated {
        provider: Provider::Claude,
        directory: HostPath(text("/work")),
        by: phone(),
        model: None,
        effort: None,
    });
    let user = event(EventKind::UserMessage {
        text: text("edit"),
        by: phone(),
    });
    let unknown = checkpoint(
        1,
        HistoryAnchor::Unknown,
        UndoAvailability::Unavailable {
            reason: text("boundary not observed"),
        },
    );
    let fork = HistoryAnchor::ClaudeFork {
        source: text("source"),
        through: text("boundary"),
        target: text("target"),
    };
    let forked = restored(
        &[
            created.clone(),
            event(EventKind::UndoHistoryRestored {
                checkpoint: CheckpointId(2),
                history: fork.clone(),
            }),
            user.clone(),
            unknown.clone(),
        ],
        Some("target"),
    );
    assert_eq!(forked.book.resume, Some(fork));
    assert_eq!(forked.conversation, Some(text("target")));
    let fresh = event(EventKind::UndoHistoryRestored {
        checkpoint: CheckpointId(2),
        history: HistoryAnchor::Fresh,
    });
    let unsent = restored(&[created.clone(), fresh.clone()], Some("new-id"));
    assert_eq!(unsent.book.resume, Some(HistoryAnchor::Fresh));
    let sent = restored(&[created, fresh, user, unknown], Some("new-id"));
    assert_eq!(
        sent.book.resume,
        Some(HistoryAnchor::Claude {
            conversation: text("new-id"),
            message: None
        })
    );
}

fn approval(for_session: Option<&str>) -> Asked {
    Asked::Approval {
        call: ToolCallId(text("toolu_1")),
        for_session: for_session.map(String::from),
    }
}

#[test]
fn a_reply_must_fit_what_the_request_asked() {
    let one = Vec::from([Answer(Vec::from([text("Blue")]))]);
    assert_eq!(
        [
            fits(&approval(None), &Reply::Decide(Decision::ApproveForSession)),
            fits(
                &approval(Some("Bash(ls)")),
                &Reply::Decide(Decision::ApproveForSession)
            ),
            fits(&approval(None), &Reply::Answer(one.clone())),
            fits(&Asked::Question { count: 1 }, &Reply::Answer(one)),
            fits(&Asked::Question { count: 2 }, &Reply::Answer(Vec::new())),
        ],
        [
            Err(text("the agent offers no approval for the session here")),
            Ok(()),
            Err(text("an approval takes a decision, not answers")),
            Ok(()),
            Err(text("0 answers for 2 questions")),
        ]
    );
}
