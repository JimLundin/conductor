use agents::runtime::{Anchoring, Command, Outcome, Start, Started};
use agents::{NativeRequest, NativeTurn, PlanStep, Reply, Step};
use proto::{
    Answer, Checkpoint, CheckpointId, ClientName, CloseReason, CommandResult,
    Decision, Effort, Event, EventKind, HistoryAnchor, HostPath, HostSeq,
    InstanceId, Kept, ModelChoice, ModelId, Models, Provider, Question,
    Rejection, RequestId, Resolution, Selection, SessionId, SubagentActivity,
    SubagentEvent, SubagentId, SubagentOutcome, Task, TaskId, TaskStatus,
    ToolCallId, ToolKind, ToolOutcome, ToolPhase, TurnOutcome,
    UndoAvailability, Workspace,
};

use super::{Asked, Call, Choice, Effect, Input, Order, Session, fits};

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

fn agent(command: Command) -> Effect {
    Effect::Call(Call::Agent(command))
}

fn launching(
    directory: &str,
    history: Option<HistoryAnchor>,
    conversation: Option<&str>,
) -> Effect {
    agent(Command::Start(Start {
        instructions: None,
        session: SessionId(1),
        directory: HostPath(text(directory)),
        model: None,
        effort: None,
        history,
        conversation: conversation.map(String::from),
    }))
}

fn started(conversation: &str, anchoring: Anchoring) -> Input {
    Input::Agent(Outcome::Started(Ok(Started {
        conversation: text(conversation),
        anchoring,
    })))
}

fn start() -> Input {
    Input::Order(Order::Start {
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
    agent(Command::Send {
        text: text(message),
        effort: None,
    })
}

fn sent() -> Input {
    Input::Agent(Outcome::Sent(Ok(())))
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
            (start(), Vec::from([launching("/work", None, None)])),
            (
                started("conv", Anchoring::Messages),
                Vec::from([
                    saved(),
                    Effect::Emit(EventKind::SessionReady),
                    accepted(),
                ]),
            ),
        ]),
    )
}

/// A session whose turn has begun.
fn running() -> Session {
    expect(
        ready(),
        Vec::from([
            (send("hi"), Vec::from([record("hi")])),
            (Input::Recorded(HostSeq(4)), Vec::from([sending("hi")])),
            (sent(), Vec::from([saved(), accepted()])),
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
            (sent(), Vec::from([saved(), accepted()])),
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
        restored(&in_workspace(), None),
        Vec::from([
            (start(), Vec::from([launching("/work", None, None)])),
            (
                started("conv", Anchoring::Messages),
                Vec::from([
                    saved(),
                    Effect::Emit(EventKind::SessionReady),
                    accepted(),
                ]),
            ),
            (send("hi"), Vec::from([record("hi")])),
            (Input::Recorded(HostSeq(4)), Vec::from([sending("hi")])),
            (
                Input::Agent(Outcome::Sent(Err(text("broken pipe")))),
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
            (Input::Recorded(HostSeq(7)), Vec::from([sending("again")])),
            (sent(), Vec::from([saved(), accepted()])),
            (
                turn_started(),
                Vec::from([
                    saved(),
                    Effect::Emit(EventKind::NativeTurnStarted {
                        history: HistoryAnchor::Messages {
                            conversation: text("conv"),
                            message: None,
                        },
                        turn_start: Some(HostSeq(7)),
                    }),
                ]),
            ),
            (
                Input::Step(Step::HistoryBoundary(Some(text("message")))),
                Vec::from([Effect::Emit(EventKind::HistoryMaterialized {
                    history: HistoryAnchor::Messages {
                        conversation: text("conv"),
                        message: Some(text("message")),
                    },
                })]),
            ),
            (
                turn_ended(),
                Vec::from([
                    Effect::Emit(EventKind::CheckpointReserved {
                        checkpoint: CheckpointId(1),
                    }),
                    Effect::Barrier,
                    Effect::Call(Call::Snapshot {
                        workspace: workspace(),
                        checkpoint: CheckpointId(1),
                        previous: Some(text("c0")),
                    }),
                ]),
            ),
            (
                Input::Snapshotted(Ok(text("c1"))),
                Vec::from([
                    Effect::Emit(EventKind::CheckpointCreated {
                        checkpoint: Checkpoint {
                            id: CheckpointId(1),
                            parent: Some(CheckpointId(0)),
                            turn_start: Some(HostSeq(7)),
                            commit: text("c1"),
                            history: HistoryAnchor::Messages {
                                conversation: text("conv"),
                                message: Some(text("message")),
                            },
                            undo: UndoAvailability::Available,
                        },
                    }),
                    completed(),
                ]),
            ),
        ]),
    );
}

/// A sub-agent's call asks like the agent's own, so a denied one that
/// fails was declined.
#[test]
fn a_subagent_is_logged_and_its_denied_call_declined() {
    let deny = || Reply::Decide(Decision::Deny);
    let agent_id = || SubagentId(text("a1"));
    let finished = |outcome| SubagentActivity::ToolCall {
        call: ToolCallId(text("toolu_1")),
        phase: ToolPhase::Finished { outcome },
    };
    let call = || ToolCallId(text("toolu_agent"));
    let _ = expect(
        running(),
        Vec::from([
            (
                Input::Step(Step::SubagentStarted {
                    agent: agent_id(),
                    call: call(),
                    description: text("look around"),
                }),
                Vec::from([Effect::Emit(EventKind::Subagent {
                    agent: agent_id(),
                    event: SubagentEvent::Started {
                        call: call(),
                        description: text("look around"),
                    },
                })]),
            ),
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
                    agent(Command::Reply {
                        native: NativeRequest(text("perm-1")),
                        reply: deny(),
                        by: phone(),
                    }),
                ]),
            ),
            (
                Input::Agent(Outcome::Replied(Ok(()))),
                Vec::from([accepted()]),
            ),
            (
                Input::Step(Step::SubagentActivity {
                    agent: agent_id(),
                    activity: finished(ToolOutcome::Failed),
                }),
                Vec::from([Effect::Emit(EventKind::Subagent {
                    agent: agent_id(),
                    event: SubagentEvent::Activity(finished(
                        ToolOutcome::Declined,
                    )),
                })]),
            ),
            (
                Input::Step(Step::SubagentEnded {
                    agent: agent_id(),
                    outcome: SubagentOutcome::Stopped,
                    summary: None,
                }),
                Vec::from([Effect::Emit(EventKind::Subagent {
                    agent: agent_id(),
                    event: SubagentEvent::Ended {
                        outcome: SubagentOutcome::Stopped,
                        summary: None,
                    },
                })]),
            ),
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
                    agent(Command::Reply {
                        native: NativeRequest(text("perm-1")),
                        reply: deny(),
                        by: phone(),
                    }),
                ]),
            ),
            (
                Input::Agent(Outcome::Replied(Ok(()))),
                Vec::from([accepted()]),
            ),
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
    let interrupting = || agent(Command::Interrupt(NativeTurn(text("conv"))));
    let interrupted = |outcome: Result<(), String>| {
        Input::Agent(Outcome::Interrupted(outcome))
    };
    let _ = expect(
        ready(),
        Vec::from([
            (interrupt(), Vec::from([rejected(Rejection::NoTurnRunning)])),
            (send("hi"), Vec::from([record("hi")])),
            (Input::Recorded(HostSeq(4)), Vec::from([sending("hi")])),
            (sent(), Vec::from([saved(), accepted()])),
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
                interrupted(Err(text("gone"))),
                Vec::from([Effect::Emit(EventKind::Error {
                    message: text("gone"),
                })]),
            ),
            (interrupt(), Vec::from([interrupting()])),
            (interrupted(Ok(())), Vec::from([accepted()])),
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
                set(Effort::Low),
                Vec::from([
                    Effect::Emit(EventKind::EffortChanged {
                        effort: Effort::Low,
                    }),
                    accepted(),
                ]),
            ),
            (
                set(Effort::High),
                Vec::from([
                    Effect::Emit(EventKind::EffortChanged {
                        effort: Effort::High,
                    }),
                    accepted(),
                ]),
            ),
            (
                set(Effort::Medium),
                Vec::from([rejected(Rejection::NotOffered {
                    reason: text("Opus doesn't take the effort medium"),
                })]),
            ),
            (send("hi"), Vec::from([record("hi")])),
            (
                Input::Recorded(HostSeq(4)),
                Vec::from([agent(Command::Send {
                    text: text("hi"),
                    effort: Some(Effort::High),
                })]),
            ),
            (sent(), Vec::from([saved(), accepted()])),
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
                Input::Step(Step::Closed(CloseReason::Exited {
                    status: text("exit status: 1"),
                })),
                Vec::from([Effect::Emit(EventKind::SessionClosed {
                    reason: CloseReason::Exited {
                        status: text("exit status: 1"),
                    },
                })]),
            ),
            (send("hi"), Vec::from([rejected(Rejection::SessionClosed)])),
            (start(), Vec::from([launching("/work", None, Some("conv"))])),
            (
                started("conv", Anchoring::Messages),
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
fn a_start_asks_for_the_history_to_restore_and_one_that_fails_closes() {
    let anchored = HistoryAnchor::Messages {
        conversation: text("conv"),
        message: None,
    };
    let history = [event(EventKind::HistoryMaterialized {
        history: anchored.clone(),
    })];
    let _ = expect(
        restored(&history, Some("conv")),
        Vec::from([
            (
                start(),
                Vec::from([launching("/work", Some(anchored), Some("conv"))]),
            ),
            (
                Input::Agent(Outcome::Started(Err(text("missing")))),
                Vec::from([
                    Effect::Emit(EventKind::SessionClosed {
                        reason: CloseReason::FailedToStart {
                            message: text("missing"),
                        },
                    }),
                    accepted(),
                ]),
            ),
            (send("hi"), Vec::from([rejected(Rejection::SessionClosed)])),
        ]),
    );
}

#[test]
fn an_agent_anchored_at_turns_materializes_its_history_as_each_starts() {
    let thread = |turn: Option<&str>| HistoryAnchor::Turns {
        conversation: text("thread"),
        turn: turn.map(String::from),
    };
    let _ = expect(
        restored(
            &[event(EventKind::WorkspaceCreated {
                workspace: workspace(),
            })],
            None,
        ),
        Vec::from([
            (start(), Vec::from([launching("/work", None, None)])),
            (
                started("thread", Anchoring::Turns),
                Vec::from([
                    Effect::Conversation(text("thread")),
                    Effect::Emit(EventKind::SessionReady),
                    accepted(),
                ]),
            ),
            (
                Input::Step(Step::TurnStarted(NativeTurn(text("t1")))),
                Vec::from([
                    Effect::Conversation(text("thread")),
                    Effect::Emit(EventKind::HistoryMaterialized {
                        history: thread(Some("t1")),
                    }),
                    Effect::Emit(EventKind::NativeTurnStarted {
                        history: thread(Some("t1")),
                        turn_start: None,
                    }),
                ]),
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

fn undo() -> Input {
    Input::Order(Order::Undo {
        checkpoint: CheckpointId(1),
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
            HistoryAnchor::Messages {
                conversation: text("conv"),
                message: Some(text("m1")),
            },
            UndoAvailability::Available,
        ),
    ];
    let _ = expect(
        restored(&history, None),
        Vec::from([
            (undo(), Vec::from([preflight()])),
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
            (Input::Exists(true), Vec::from([agent(Command::Shutdown)])),
            (
                Input::Agent(Outcome::ShutDown(Ok(()))),
                Vec::from([launching(
                    "/work",
                    Some(HistoryAnchor::Fresh),
                    None,
                )]),
            ),
            (
                started("new", Anchoring::Messages),
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
            (
                Input::Order(Order::Undo {
                    checkpoint: CheckpointId(0),
                    fork: text("fork"),
                }),
                Vec::from([rejected(Rejection::UndoInProgress)]),
            ),
            (undo(), Vec::from([restore()])),
            (ok(), undone()),
            (
                undo(),
                Vec::from([rejected(Rejection::CheckpointUnavailable)]),
            ),
        ]),
    );
}

fn thread(turn: &str) -> HistoryAnchor {
    HistoryAnchor::Turns {
        conversation: text("thread"),
        turn: Some(text(turn)),
    }
}

fn turns(listed: &[&str]) -> Input {
    Input::Agent(Outcome::Turns(Ok(listed
        .iter()
        .copied()
        .map(String::from)
        .collect())))
}

/// A session anchored at turns whose directory is gone, on its way to
/// undo checkpoint 1 in the worktree once it lists its turns.
fn dispatch_revert() -> Vec<Effect> {
    Vec::from([
        Effect::Emit(EventKind::UndoRevertDispatched {
            checkpoint: CheckpointId(1),
        }),
        Effect::Barrier,
        agent(Command::Revert(text("t1"))),
    ])
}

fn reverting() -> Session {
    let history = [
        event(EventKind::WorkspaceCreated {
            workspace: workspace(),
        }),
        checkpoint(0, thread("t0"), UndoAvailability::Available),
        checkpoint(1, thread("t1"), UndoAvailability::Available),
    ];
    expect(
        restored(&history, None),
        Vec::from([
            (undo(), Vec::from([preflight()])),
            (
                ok(),
                Vec::from([
                    Effect::Emit(EventKind::UndoStarted {
                        checkpoint: CheckpointId(1),
                        restore: CheckpointId(0),
                        history: thread("t1"),
                    }),
                    Effect::Barrier,
                    Effect::Call(Call::Exists(HostPath(text("/work")))),
                ]),
            ),
            (
                Input::Exists(false),
                Vec::from([launching(
                    "/worktree",
                    Some(HistoryAnchor::Turns {
                        conversation: text("thread"),
                        turn: None,
                    }),
                    None,
                )]),
            ),
            (
                started("thread", Anchoring::Turns),
                Vec::from([agent(Command::Turns)]),
            ),
        ]),
    )
}

#[test]
fn an_undo_in_a_gone_directory_reverts_the_turns_in_the_worktree() {
    let _ = expect(
        reverting(),
        Vec::from([
            (turns(&["t0", "t1"]), dispatch_revert()),
            (
                Input::Agent(Outcome::Reverted(Ok(()))),
                Vec::from([agent(Command::Turns)]),
            ),
            (turns(&["t0"]), Vec::from([agent(Command::Shutdown)])),
            (
                Input::Agent(Outcome::ShutDown(Ok(()))),
                Vec::from([
                    history_restored(thread("t0")),
                    Effect::Call(Call::History),
                ]),
            ),
            (
                Input::History(Vec::new()),
                Vec::from([Effect::Barrier, restore()]),
            ),
            (
                ok(),
                Vec::from([launching("/work", Some(thread("t0")), None)]),
            ),
            (started("thread", Anchoring::Turns), undone()),
        ]),
    );
}

#[test]
fn an_unknown_rollback_reopens_and_verifies_history_before_continuing() {
    let _ = expect(
        reverting(),
        Vec::from([
            (turns(&["t0", "t1"]), dispatch_revert()),
            (
                Input::Agent(Outcome::RevertUnknown),
                Vec::from([launching(
                    "/worktree",
                    Some(HistoryAnchor::Turns {
                        conversation: text("thread"),
                        turn: None,
                    }),
                    None,
                )]),
            ),
            (
                started("thread", Anchoring::Turns),
                Vec::from([agent(Command::Turns)]),
            ),
            (turns(&["t0"]), Vec::from([agent(Command::Shutdown)])),
            (
                Input::Agent(Outcome::ShutDown(Ok(()))),
                Vec::from([
                    history_restored(thread("t0")),
                    Effect::Call(Call::History),
                ]),
            ),
        ]),
    );
}

#[test]
fn a_replayed_unverified_rollback_reads_history_without_sending_again() {
    let session = restored(
        &[
            event(EventKind::WorkspaceCreated {
                workspace: workspace(),
            }),
            checkpoint(0, thread("t0"), UndoAvailability::Available),
            checkpoint(1, thread("t1"), UndoAvailability::Available),
            event(EventKind::UndoStarted {
                checkpoint: CheckpointId(1),
                restore: CheckpointId(0),
                history: thread("t1"),
            }),
            event(EventKind::UndoRevertDispatched {
                checkpoint: CheckpointId(1),
            }),
        ],
        None,
    );
    let verify = || {
        Vec::from([
            (
                undo(),
                Vec::from([Effect::Call(Call::Exists(HostPath(text(
                    "/work",
                ))))]),
            ),
            (
                Input::Exists(true),
                Vec::from([launching(
                    "/work",
                    Some(HistoryAnchor::Turns {
                        conversation: text("thread"),
                        turn: None,
                    }),
                    None,
                )]),
            ),
            (
                started("thread", Anchoring::Turns),
                Vec::from([agent(Command::Turns)]),
            ),
        ])
    };
    let checking = expect(session, verify());
    let unresolved = expect(
        checking,
        Vec::from([(
            turns(&["t0", "t1"]),
            Vec::from([
                Effect::Emit(EventKind::Error {
                    message: text(
                        "could not verify the native rollback outcome: native history does not match the retained turn prefix",
                    ),
                }),
                rejected(Rejection::UndoInProgress),
            ]),
        )]),
    );
    let retry = expect(unresolved, verify());
    let _ = expect(
        retry,
        Vec::from([(
            turns(&["t0"]),
            Vec::from([
                history_restored(thread("t0")),
                Effect::Call(Call::History),
            ]),
        )]),
    );
}

/// A retried undo whose turn the agent already reverted only checks the
/// turns left.
#[test]
fn an_undo_does_not_revert_a_turn_the_agent_no_longer_lists() {
    let _ = expect(
        reverting(),
        Vec::from([
            (turns(&["t0"]), Vec::from([agent(Command::Turns)])),
            (turns(&["t0"]), Vec::from([agent(Command::Shutdown)])),
        ]),
    );
}

#[test]
fn an_undo_to_a_message_forks_the_conversation_through_it() {
    let message = |id: &str| HistoryAnchor::Messages {
        conversation: text("conv"),
        message: Some(text(id)),
    };
    let fork = HistoryAnchor::Fork {
        source: text("conv"),
        through: text("m0"),
        target: text("fork"),
    };
    let history = [
        event(EventKind::WorkspaceCreated {
            workspace: workspace(),
        }),
        checkpoint(0, message("m0"), UndoAvailability::Available),
        checkpoint(1, message("m1"), UndoAvailability::Available),
    ];
    let _ = expect(
        restored(&history, None),
        Vec::from([
            (undo(), Vec::from([preflight()])),
            (
                ok(),
                Vec::from([
                    Effect::Emit(EventKind::UndoStarted {
                        checkpoint: CheckpointId(1),
                        restore: CheckpointId(0),
                        history: fork.clone(),
                    }),
                    Effect::Barrier,
                    Effect::Call(Call::Exists(HostPath(text("/work")))),
                ]),
            ),
            (Input::Exists(true), Vec::from([agent(Command::Shutdown)])),
            (
                Input::Agent(Outcome::ShutDown(Ok(()))),
                Vec::from([launching("/work", Some(fork), None)]),
            ),
        ]),
    );
}

fn tasks(label: &str, status: TaskStatus) -> Vec<Task> {
    Vec::from([Task {
        id: TaskId(7),
        text: text(label),
        status,
        holder: SessionId(1),
    }])
}

/// An undo restores the session's tasks as they were when the checkpoint
/// it goes back to was taken.
#[test]
fn the_tasks_before_a_checkpoint_are_the_session_s_last_before_it() {
    let at = |seq, session, kind| Event {
        seq: HostSeq(seq),
        session: SessionId(session),
        kind,
    };
    let changed = |seq, session, label| {
        at(
            seq,
            session,
            EventKind::TasksChanged {
                tasks: tasks(label, TaskStatus::Pending),
            },
        )
    };
    let taken = |seq, session, id| Event {
        session: SessionId(session),
        seq: HostSeq(seq),
        ..checkpoint(id, HistoryAnchor::Fresh, UndoAvailability::Available)
    };
    let history = [
        changed(1, 1, "first"),
        taken(2, 2, 1),
        changed(3, 1, "second"),
        changed(4, 2, "another session's"),
        taken(5, 1, 1),
        changed(6, 1, "after"),
    ];
    assert_eq!(
        [CheckpointId(1), CheckpointId(9)].map(|id| super::tasks_before(
            &history,
            SessionId(1),
            id
        )),
        [tasks("second", TaskStatus::Pending), Vec::new()]
    );
}

#[test]
fn a_restored_session_keeps_its_tasks_and_their_ids() {
    let checklist = |status| {
        Input::Step(Step::Checklist(Vec::from([PlanStep {
            text: text("plan"),
            status,
        }])))
    };
    let _ = expect(
        restored(
            &[event(EventKind::TasksChanged {
                tasks: tasks("plan", TaskStatus::Pending),
            })],
            None,
        ),
        Vec::from([
            (checklist(TaskStatus::Pending), Vec::new()),
            (
                checklist(TaskStatus::Completed),
                Vec::from([Effect::Emit(EventKind::TasksChanged {
                    tasks: tasks("plan", TaskStatus::Completed),
                })]),
            ),
        ]),
    );
}

fn asking(native: &str, call: &str) -> Input {
    Input::Step(Step::Open {
        native: NativeRequest(text(native)),
        call: ToolCallId(text(call)),
        tool: ToolKind::Command {
            command: text("ls"),
        },
        reason: None,
        for_session: None,
    })
}

fn opened(request: u64, call: &str) -> Effect {
    Effect::Emit(EventKind::RequestOpened {
        request: RequestId(request),
        call: ToolCallId(text(call)),
        tool: ToolKind::Command {
            command: text("ls"),
        },
        reason: None,
        for_session: None,
    })
}

fn withdrawn(request: u64) -> Effect {
    Effect::Emit(EventKind::RequestResolved {
        request: RequestId(request),
        resolution: Resolution::Withdrawn,
    })
}

#[test]
fn an_agent_withdraws_only_the_request_it_names() {
    let withdraw =
        |native: &str| Input::Step(Step::Withdraw(NativeRequest(text(native))));
    let _ = expect(
        running(),
        Vec::from([
            (
                asking("perm-1", "toolu_1"),
                Vec::from([opened(1, "toolu_1")]),
            ),
            (
                asking("perm-2", "toolu_2"),
                Vec::from([opened(2, "toolu_2")]),
            ),
            (withdraw("perm-2"), Vec::from([withdrawn(2)])),
            (withdraw("perm-2"), Vec::new()),
            (withdraw("perm-1"), Vec::from([withdrawn(1)])),
        ]),
    );
}

#[test]
fn each_denied_call_is_declined_when_it_fails() {
    let deny = |request, native: &str| {
        (
            reply(request, Reply::Decide(Decision::Deny), "phone"),
            Vec::from([
                Effect::Emit(EventKind::RequestResolved {
                    request: RequestId(request),
                    resolution: Resolution::Decided {
                        decision: Decision::Deny,
                        by: phone(),
                    },
                }),
                agent(Command::Reply {
                    native: NativeRequest(text(native)),
                    reply: Reply::Decide(Decision::Deny),
                    by: phone(),
                }),
            ]),
        )
    };
    let failed = |call: &str| {
        (
            Input::Step(Step::ToolFinished {
                call: ToolCallId(text(call)),
                outcome: ToolOutcome::Failed,
            }),
            Vec::from([Effect::Emit(EventKind::ToolCall {
                call: ToolCallId(text(call)),
                phase: ToolPhase::Finished {
                    outcome: ToolOutcome::Declined,
                },
            })]),
        )
    };
    let replied = || {
        (
            Input::Agent(Outcome::Replied(Ok(()))),
            Vec::from([accepted()]),
        )
    };
    let _ = expect(
        running(),
        Vec::from([
            (
                asking("perm-1", "toolu_1"),
                Vec::from([opened(1, "toolu_1")]),
            ),
            (
                asking("perm-2", "toolu_2"),
                Vec::from([opened(2, "toolu_2")]),
            ),
            deny(1, "perm-1"),
            replied(),
            deny(2, "perm-2"),
            replied(),
            failed("toolu_1"),
            failed("toolu_2"),
        ]),
    );
}

/// The ids of requests the log holds stay taken, and a resolved one keeps
/// who resolved it.
#[test]
fn a_restored_session_keeps_its_request_ids_and_who_answered_them() {
    let history = [
        event(EventKind::RequestOpened {
            request: RequestId(1),
            call: ToolCallId(text("toolu_1")),
            tool: ToolKind::Command {
                command: text("ls"),
            },
            reason: None,
            for_session: None,
        }),
        event(EventKind::RequestResolved {
            request: RequestId(1),
            resolution: Resolution::Decided {
                decision: Decision::Deny,
                by: phone(),
            },
        }),
        event(EventKind::QuestionAsked {
            request: RequestId(2),
            questions: Vec::new(),
        }),
    ];
    let _ = expect(
        restored(&history, None),
        Vec::from([
            (start(), Vec::from([launching("/work", None, None)])),
            (
                started("conv", Anchoring::Messages),
                Vec::from([
                    saved(),
                    Effect::Emit(EventKind::SessionReady),
                    accepted(),
                ]),
            ),
            (
                reply(1, Reply::Decide(Decision::Approve), "laptop"),
                Vec::from([rejected(Rejection::AlreadyResolved {
                    by: phone(),
                })]),
            ),
            (
                reply(2, Reply::Answer(Vec::new()), "phone"),
                Vec::from([rejected(Rejection::UnknownRequest)]),
            ),
            (
                asking("perm-3", "toolu_3"),
                Vec::from([opened(3, "toolu_3")]),
            ),
        ]),
    );
}

/// A model the credential no longer offers gives way to the default, but
/// the default itself stays even when the CLI doesn't list it.
#[test]
fn a_start_keeps_the_chosen_model_only_while_it_is_offered() {
    let model = |id: &str| Some(ModelId(text(id)));
    let offered = Models {
        choices: Vec::from([ModelChoice {
            id: ModelId(text("big")),
            name: text("Big"),
            description: String::new(),
            efforts: Vec::new(),
        }]),
        default: model("house"),
    };
    let choosing = |id: &str| {
        Session::restore(
            SessionId(1),
            HostPath(text("/work")),
            Choice {
                model: model(id),
                effort: None,
            },
            None,
            &[],
        )
    };
    let starting = |id: &str| {
        agent(Command::Start(Start {
            instructions: None,
            session: SessionId(1),
            directory: HostPath(text("/work")),
            model: model(id),
            effort: None,
            history: None,
            conversation: None,
        }))
    };
    let order = || {
        Input::Order(Order::Start {
            offered: offered.clone(),
        })
    };
    let _ = expect(
        choosing("gone"),
        Vec::from([(
            order(),
            Vec::from([
                Effect::Emit(EventKind::ModelChanged {
                    model: model("house"),
                    effort: None,
                    reason: text(
                        "Model changed to house: gone is not offered by the current credential",
                    ),
                }),
                starting("house"),
            ]),
        )]),
    );
    let _ = expect(
        choosing("house"),
        Vec::from([(order(), Vec::from([starting("house")]))]),
    );
}

#[test]
fn a_turn_whose_checkpoint_fails_still_ends_and_logs_why() {
    let _ = expect(
        restored(&in_workspace(), None),
        Vec::from([
            (start(), Vec::from([launching("/work", None, None)])),
            (
                started("conv", Anchoring::Messages),
                Vec::from([
                    saved(),
                    Effect::Emit(EventKind::SessionReady),
                    accepted(),
                ]),
            ),
            (send("hi"), Vec::from([record("hi")])),
            (Input::Recorded(HostSeq(4)), Vec::from([sending("hi")])),
            (sent(), Vec::from([saved(), accepted()])),
            (start(), Vec::from([accepted()])),
            (
                turn_started(),
                Vec::from([
                    saved(),
                    Effect::Emit(EventKind::NativeTurnStarted {
                        history: HistoryAnchor::Messages {
                            conversation: text("conv"),
                            message: None,
                        },
                        turn_start: Some(HostSeq(4)),
                    }),
                ]),
            ),
            (start(), Vec::from([accepted()])),
            (
                turn_ended(),
                Vec::from([
                    Effect::Emit(EventKind::CheckpointReserved {
                        checkpoint: CheckpointId(1),
                    }),
                    Effect::Barrier,
                    Effect::Call(Call::Snapshot {
                        workspace: workspace(),
                        checkpoint: CheckpointId(1),
                        previous: Some(text("c0")),
                    }),
                ]),
            ),
            (
                Input::Snapshotted(Err(text("disk full"))),
                Vec::from([
                    Effect::Emit(EventKind::Error {
                        message: text("checkpoint failed: disk full"),
                    }),
                    completed(),
                ]),
            ),
            (
                start(),
                Vec::from([
                    Effect::Emit(EventKind::CheckpointReserved {
                        checkpoint: CheckpointId(2),
                    }),
                    Effect::Barrier,
                    Effect::Call(Call::Snapshot {
                        workspace: workspace(),
                        checkpoint: CheckpointId(2),
                        previous: Some(text("c0")),
                    }),
                ]),
            ),
            (
                Input::Snapshotted(Err(text("disk full"))),
                Vec::from([rejected(Rejection::WorkspaceFailure {
                    reason: text("disk full"),
                })]),
            ),
        ]),
    );
}

#[test]
fn a_restored_session_resumes_the_fork_or_the_conversation_saved_after_it() {
    let created = event(EventKind::SessionCreated {
        instance: InstanceId::own(Provider::Claude),
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
    let fork = HistoryAnchor::Fork {
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
    assert_eq!(forked.book.resume(true), Some(fork));
    assert_eq!(forked.conversation, Some(text("target")));
    let fresh = event(EventKind::UndoHistoryRestored {
        checkpoint: CheckpointId(2),
        history: HistoryAnchor::Fresh,
    });
    let unsent = restored(&[created.clone(), fresh.clone()], Some("new-id"));
    assert_eq!(unsent.book.resume(true), Some(HistoryAnchor::Fresh));
    let _ = expect(
        restored(&[created, fresh, user, unknown], Some("new-id")),
        Vec::from([(
            start(),
            Vec::from([launching("/work", None, Some("new-id"))]),
        )]),
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

fn remove() -> Input {
    Input::Order(Order::Remove)
}

fn in_workspace() -> [Event; 2] {
    [
        event(EventKind::WorkspaceCreated {
            workspace: workspace(),
        }),
        checkpoint(0, HistoryAnchor::Fresh, UndoAvailability::Available),
    ]
}

#[test]
fn removing_stops_the_agent_between_turns_then_removes_the_workspace() {
    let up = expect(
        restored(&in_workspace(), None),
        Vec::from([
            (start(), Vec::from([launching("/work", None, None)])),
            (
                started("conv", Anchoring::Messages),
                Vec::from([
                    saved(),
                    Effect::Emit(EventKind::SessionReady),
                    accepted(),
                ]),
            ),
        ]),
    );
    let removed = expect(
        up,
        Vec::from([
            (remove(), Vec::from([agent(Command::Shutdown)])),
            (
                Input::Agent(Outcome::ShutDown(Ok(()))),
                Vec::from([
                    Effect::Emit(EventKind::SessionClosed {
                        reason: CloseReason::Stopped {
                            reason: text(super::REMOVED),
                        },
                    }),
                    Effect::Barrier,
                    Effect::Call(Call::Remove(workspace())),
                ]),
            ),
            (
                Input::Removed(Ok(Kept::Local)),
                Vec::from([
                    Effect::Emit(EventKind::SessionRemoved {
                        kept: Some(Kept::Local),
                    }),
                    accepted(),
                ]),
            ),
        ]),
    );
    let _ = expect(
        removed,
        Vec::from([
            (start(), Vec::from([rejected(Rejection::SessionRemoved)])),
            (remove(), Vec::from([rejected(Rejection::SessionRemoved)])),
        ]),
    );
}

#[test]
fn a_session_is_not_removed_while_a_turn_runs() {
    let _ = expect(
        running(),
        Vec::from([(
            remove(),
            Vec::from([rejected(Rejection::TurnInProgress)]),
        )]),
    );
}

#[test]
fn a_removal_that_fails_keeps_the_session_to_try_again() {
    let removing = || {
        Vec::from([Effect::Barrier, Effect::Call(Call::Remove(workspace()))])
    };
    let _ = expect(
        restored(&in_workspace(), None),
        Vec::from([
            (remove(), removing()),
            (
                Input::Removed(Err(text("worktree locked"))),
                Vec::from([
                    Effect::Emit(EventKind::Error {
                        message: text(
                            "the session was not removed: worktree locked",
                        ),
                    }),
                    rejected(Rejection::WorkspaceFailure {
                        reason: text("worktree locked"),
                    }),
                ]),
            ),
            (remove(), removing()),
        ]),
    );
}

#[test]
fn a_session_without_a_workspace_is_removed_at_once_and_stays_removed() {
    let _ = expect(
        restored(&[], None),
        Vec::from([(
            remove(),
            Vec::from([
                Effect::Emit(EventKind::SessionRemoved { kept: None }),
                accepted(),
            ]),
        )]),
    );
    let _ = expect(
        restored(&[event(EventKind::SessionRemoved { kept: None })], None),
        Vec::from([(
            start(),
            Vec::from([rejected(Rejection::SessionRemoved)]),
        )]),
    );
}

#[test]
fn a_failed_shutdown_keeps_the_agent_and_workspace_available_for_retry() {
    let stop = || Input::Order(Order::Stop(text("finished")));
    let failed = || Input::Agent(Outcome::ShutDown(Err(text("still running"))));
    let no_shutdown = || {
        Vec::from([
            Effect::Emit(EventKind::Error {
                message: text(
                    "the agent did not confirm shutdown: still running",
                ),
            }),
            rejected(Rejection::InstanceUnavailable {
                reason: text("still running"),
            }),
        ])
    };
    let _ = expect(
        ready(),
        Vec::from([
            (stop(), Vec::from([agent(Command::Shutdown)])),
            (failed(), no_shutdown()),
            (remove(), Vec::from([agent(Command::Shutdown)])),
            (failed(), no_shutdown()),
            (remove(), Vec::from([agent(Command::Shutdown)])),
        ]),
    );
}

#[test]
fn a_stop_closes_the_session_between_turns() {
    let stop = || Input::Order(Order::Stop(text("finished")));
    let _ = expect(
        ready(),
        Vec::from([
            (stop(), Vec::from([agent(Command::Shutdown)])),
            (
                Input::Agent(Outcome::ShutDown(Ok(()))),
                Vec::from([
                    Effect::Emit(EventKind::SessionClosed {
                        reason: CloseReason::Stopped {
                            reason: text("finished"),
                        },
                    }),
                    accepted(),
                ]),
            ),
            (stop(), Vec::from([accepted()])),
        ]),
    );
    let _ = expect(
        running(),
        Vec::from([(stop(), Vec::from([rejected(Rejection::TurnInProgress)]))]),
    );
}
