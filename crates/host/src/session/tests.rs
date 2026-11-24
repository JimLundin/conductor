use crate::HostPath;
use crate::log::{Event, HostSeq};
use crate::session::{
    EventKind, RequestId, Resolution, SessionId, SessionRefusal, SessionResult,
    SubagentEvent,
};
use crate::workspace::{Kept, Snapshot, Workspace};
use agents::Output;
use agents::runtime::{Command, Directory, Outcome, Start, Started};
use agents::{
    Answer, ChecklistEntry, ChecklistStatus, ClientName, CloseReason, Decision,
    Effort, ModelChoice, ModelId, Models, Question, Selection,
    SubagentActivity, SubagentId, SubagentOutcome, ToolCallId, ToolKind,
    ToolOutcome, ToolPhase, TurnOutcome,
};
use agents::{NativeRequest, NativeTurn, Reply, Step};

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

fn launching(directory: &str, conversation: Option<&str>) -> Effect {
    agent(Command::Start(Start {
        instructions: None,
        session: agents::runtime::Session(1),
        directory: Directory(text(directory)),
        model: None,
        effort: None,
        conversation: conversation.map(String::from),
    }))
}

fn started(conversation: &str) -> Input {
    Input::Agent(Outcome::Started(Ok(Started {
        conversation: text(conversation),
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

fn accepted() -> Effect {
    Effect::Reply(SessionResult::Accepted)
}

fn rejected(rejection: SessionRefusal) -> Effect {
    Effect::Reply(SessionResult::Refused(rejection))
}

fn ready() -> Session {
    expect(
        restored(&[], None),
        Vec::from([
            (start(), Vec::from([launching("/work", None)])),
            (
                started("conv"),
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
        agent: None,
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
                Vec::from([rejected(SessionRefusal::TurnInProgress)]),
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
            (start(), Vec::from([launching("/work", None)])),
            (
                started("conv"),
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
                        turn_start: Some(HostSeq(7)),
                    }),
                ]),
            ),
            (
                turn_ended(),
                Vec::from([
                    Effect::Barrier,
                    Effect::Call(Call::Snapshot(workspace())),
                ]),
            ),
            (
                Input::Snapshotted(Ok(text("c1"))),
                Vec::from([Effect::Call(Call::Record(
                    EventKind::SnapshotRecorded {
                        snapshot: Snapshot {
                            turn_start: Some(HostSeq(7)),
                            commit: text("c1"),
                        },
                    },
                ))]),
            ),
            (Input::Recorded(HostSeq(11)), Vec::from([completed()])),
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
                    kind: None,
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
                Input::Step(Step::Open {
                    agent: Some(agent_id()),
                    native: NativeRequest(text("perm-1")),
                    call: ToolCallId(text("toolu_1")),
                    tool: ToolKind::Command {
                        command: text("ls"),
                    },
                    reason: None,
                    for_session: None,
                }),
                Vec::from([Effect::Emit(EventKind::Subagent {
                    agent: agent_id(),
                    event: SubagentEvent::RequestOpened {
                        request: RequestId(1),
                        call: ToolCallId(text("toolu_1")),
                        tool: ToolKind::Command {
                            command: text("ls"),
                        },
                        reason: None,
                        for_session: None,
                    },
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
                Vec::from([rejected(SessionRefusal::AlreadyResolved {
                    by: phone(),
                })]),
            ),
            (
                Input::Step(Step::ToolFinished {
                    call: ToolCallId(text("toolu_1")),
                    outcome: ToolOutcome::Failed,
                }),
                Vec::from([Effect::Emit(EventKind::Agent(Output::ToolCall {
                    call: ToolCallId(text("toolu_1")),
                    phase: ToolPhase::Finished {
                        outcome: ToolOutcome::Declined,
                    },
                }))]),
            ),
            (
                Input::Step(Step::Ask {
                    agent: None,
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
                Vec::from([rejected(SessionRefusal::UnknownRequest)]),
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
                    agent: None,
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
                Vec::from([rejected(SessionRefusal::WrongReply {
                    reason: text("a question takes answers, not a decision"),
                })]),
            ),
            (
                reply(9, Reply::Answer(Vec::new()), "phone"),
                Vec::from([rejected(SessionRefusal::UnknownRequest)]),
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
            (
                interrupt(),
                Vec::from([rejected(SessionRefusal::NoTurnRunning)]),
            ),
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
                Vec::from([rejected(SessionRefusal::NotOffered {
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
            (
                send("hi"),
                Vec::from([rejected(SessionRefusal::SessionClosed)]),
            ),
            (start(), Vec::from([launching("/work", Some("conv"))])),
            (
                started("conv"),
                Vec::from([
                    saved(),
                    Effect::Emit(EventKind::SessionReady),
                    accepted(),
                ]),
            ),
            (
                reply(1, Reply::Decide(Decision::Approve), "phone"),
                Vec::from([rejected(SessionRefusal::UnknownRequest)]),
            ),
        ]),
    );
}

#[test]
fn a_start_resumes_the_saved_conversation_and_one_that_fails_closes() {
    let _ = expect(
        restored(&[], Some("saved")),
        Vec::from([
            (start(), Vec::from([launching("/work", Some("saved"))])),
            (
                Input::Agent(Outcome::Started(Err(text("missing executable")))),
                Vec::from([
                    Effect::Emit(EventKind::SessionClosed {
                        reason: CloseReason::FailedToStart {
                            message: text("missing executable"),
                        },
                    }),
                    accepted(),
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

fn entries(label: &str, status: ChecklistStatus) -> Vec<ChecklistEntry> {
    Vec::from([ChecklistEntry {
        text: text(label),
        status,
    }])
}

#[test]
fn a_restored_session_keeps_its_checklist() {
    let checklist = |status| {
        Input::Step(Step::Checklist(Vec::from([ChecklistEntry {
            text: text("plan"),
            status,
        }])))
    };
    let _ = expect(
        restored(
            &[event(EventKind::ChecklistChanged {
                checklist: entries("plan", ChecklistStatus::Pending),
            })],
            None,
        ),
        Vec::from([
            (checklist(ChecklistStatus::Pending), Vec::new()),
            (
                checklist(ChecklistStatus::Completed),
                Vec::from([Effect::Emit(EventKind::ChecklistChanged {
                    checklist: entries("plan", ChecklistStatus::Completed),
                })]),
            ),
        ]),
    );
}

#[test]
fn main_and_child_checklists_replace_their_own_ordered_snapshots() {
    let first = entries("read", ChecklistStatus::Pending);
    let rewritten = Vec::from([
        ChecklistEntry {
            text: text("test"),
            status: ChecklistStatus::Completed,
        },
        ChecklistEntry {
            text: text("read"),
            status: ChecklistStatus::InProgress,
        },
        ChecklistEntry {
            text: text("test"),
            status: ChecklistStatus::Pending,
        },
    ]);
    let child = SubagentId(text("child"));
    let child_step = |checklist| {
        Input::Step(Step::SubagentChecklist {
            agent: child.clone(),
            steps: checklist,
        })
    };
    let changed =
        |checklist| Effect::Emit(EventKind::ChecklistChanged { checklist });
    let child_changed = |checklist| {
        Effect::Emit(EventKind::Subagent {
            agent: child.clone(),
            event: SubagentEvent::ChecklistChanged { checklist },
        })
    };
    let _ = expect(
        ready(),
        Vec::from([
            (
                Input::Step(Step::Checklist(first.clone())),
                Vec::from([changed(first.clone())]),
            ),
            (
                child_step(rewritten.clone()),
                Vec::from([child_changed(rewritten.clone())]),
            ),
            (Input::Step(Step::Checklist(first)), Vec::new()),
            (
                Input::Step(Step::Checklist(rewritten.clone())),
                Vec::from([changed(rewritten.clone())]),
            ),
            (
                child_step(Vec::new()),
                Vec::from([child_changed(Vec::new())]),
            ),
            (Input::Step(Step::Checklist(rewritten)), Vec::new()),
            (
                Input::Step(Step::Checklist(Vec::new())),
                Vec::from([changed(Vec::new())]),
            ),
        ]),
    );
}

fn asking(native: &str, call: &str) -> Input {
    Input::Step(Step::Open {
        agent: None,
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
            Vec::from([Effect::Emit(EventKind::Agent(Output::ToolCall {
                call: ToolCallId(text(call)),
                phase: ToolPhase::Finished {
                    outcome: ToolOutcome::Declined,
                },
            }))]),
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
            (start(), Vec::from([launching("/work", None)])),
            (
                started("conv"),
                Vec::from([
                    saved(),
                    Effect::Emit(EventKind::SessionReady),
                    accepted(),
                ]),
            ),
            (
                reply(1, Reply::Decide(Decision::Approve), "laptop"),
                Vec::from([rejected(SessionRefusal::AlreadyResolved {
                    by: phone(),
                })]),
            ),
            (
                reply(2, Reply::Answer(Vec::new()), "phone"),
                Vec::from([rejected(SessionRefusal::UnknownRequest)]),
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
            session: agents::runtime::Session(1),
            directory: Directory(text("/work")),
            model: model(id),
            effort: None,
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
fn a_turn_whose_snapshot_fails_still_ends_and_logs_why() {
    let _ = expect(
        restored(&in_workspace(), None),
        Vec::from([
            (start(), Vec::from([launching("/work", None)])),
            (
                started("conv"),
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
                        turn_start: Some(HostSeq(4)),
                    }),
                ]),
            ),
            (start(), Vec::from([accepted()])),
            (
                turn_ended(),
                Vec::from([
                    Effect::Barrier,
                    Effect::Call(Call::Snapshot(workspace())),
                ]),
            ),
            (
                Input::Snapshotted(Err(text("disk full"))),
                Vec::from([
                    Effect::Emit(EventKind::Error {
                        message: text("snapshot failed: disk full"),
                    }),
                    completed(),
                ]),
            ),
            (
                start(),
                Vec::from([
                    Effect::Barrier,
                    Effect::Call(Call::Snapshot(workspace())),
                ]),
            ),
            (
                Input::Snapshotted(Err(text("disk full"))),
                Vec::from([rejected(SessionRefusal::WorkspaceFailure {
                    reason: text("disk full"),
                })]),
            ),
        ]),
    );
}

fn approval(for_session: Option<&str>) -> Asked {
    Asked::Approval {
        agent: None,
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
            fits(
                &Asked::Question {
                    count: 1,
                    agent: None
                },
                &Reply::Answer(one)
            ),
            fits(
                &Asked::Question {
                    count: 2,
                    agent: None
                },
                &Reply::Answer(Vec::new())
            ),
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
        event(EventKind::SnapshotRecorded {
            snapshot: Snapshot {
                turn_start: None,
                commit: text("c0"),
            },
        }),
    ]
}

#[test]
fn removing_stops_the_agent_between_turns_then_removes_the_workspace() {
    let up = expect(
        restored(&in_workspace(), None),
        Vec::from([
            (start(), Vec::from([launching("/work", None)])),
            (
                started("conv"),
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
            (
                start(),
                Vec::from([rejected(SessionRefusal::SessionRemoved)]),
            ),
            (
                remove(),
                Vec::from([rejected(SessionRefusal::SessionRemoved)]),
            ),
        ]),
    );
}

#[test]
fn a_session_is_not_removed_while_a_turn_runs() {
    let _ = expect(
        running(),
        Vec::from([(
            remove(),
            Vec::from([rejected(SessionRefusal::TurnInProgress)]),
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
                    rejected(SessionRefusal::WorkspaceFailure {
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
            Vec::from([rejected(SessionRefusal::SessionRemoved)]),
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
            rejected(SessionRefusal::InstanceUnavailable {
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
        Vec::from([(
            stop(),
            Vec::from([rejected(SessionRefusal::TurnInProgress)]),
        )]),
    );
}

#[test]
fn a_rejection_says_why_for_a_person() {
    assert_eq!(
        SessionRefusal::NoSuchDirectory.label(),
        "no directory at that path on the host"
    );
}
