use agents::runtime::{Anchoring, Command, Outcome, Start, Started};
use agents::{NativeRequest, NativeTurn, Reply, Step};
use proto::{
    Answer, Checkpoint, CheckpointId, ClientName, CloseReason, CommandResult,
    Decision, Effort, Event, EventKind, HistoryAnchor, HostPath, HostSeq,
    InstanceId, ModelChoice, ModelId, Models, Provider, Question, Rejection,
    RequestId, Resolution, Selection, SessionId, SubagentActivity,
    SubagentEvent, SubagentId, SubagentOutcome, ToolCallId, ToolKind,
    ToolOutcome, ToolPhase, TurnOutcome, UndoAvailability, Workspace,
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
        ready(),
        Vec::from([
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
            (undo(), Vec::from([restore()])),
            (ok(), undone()),
            (
                undo(),
                Vec::from([rejected(Rejection::CheckpointUnavailable)]),
            ),
        ]),
    );
}

#[test]
fn an_undo_in_a_gone_directory_reverts_the_turns_in_the_worktree() {
    let thread = |turn: &str| HistoryAnchor::Turns {
        conversation: text("thread"),
        turn: Some(text(turn)),
    };
    let history = [
        event(EventKind::WorkspaceCreated {
            workspace: workspace(),
        }),
        checkpoint(0, thread("t0"), UndoAvailability::Available),
        checkpoint(1, thread("t1"), UndoAvailability::Available),
    ];
    let turns = |listed: &[&str]| {
        Input::Agent(Outcome::Turns(Ok(listed
            .iter()
            .copied()
            .map(String::from)
            .collect())))
    };
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
            (
                turns(&["t0", "t1"]),
                Vec::from([agent(Command::Revert(text("t1")))]),
            ),
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
fn a_restored_session_resumes_the_fork_or_the_conversation_saved_after_it() {
    let created = event(EventKind::SessionCreated {
        instance: InstanceId::named("claude"),
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
    assert_eq!(forked.book.resume, Some(fork));
    assert_eq!(forked.conversation, Some(text("target")));
    let fresh = event(EventKind::UndoHistoryRestored {
        checkpoint: CheckpointId(2),
        history: HistoryAnchor::Fresh,
    });
    let unsent = restored(&[created.clone(), fresh.clone()], Some("new-id"));
    assert_eq!(unsent.book.resume, Some(HistoryAnchor::Fresh));
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
