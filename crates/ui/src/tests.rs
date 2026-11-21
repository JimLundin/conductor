mod credentials;
mod hosts;
mod new_session;
mod panes;
mod places;
mod projects;

use alloc::collections::BTreeMap;
use alloc::rc::Rc;

use client::link::{LinkState, Status, Update};
use proto::StartAt;
use proto::{
    Auth, AuthToken, Availability, Catalog, Checkpoint, CheckpointDiff,
    CheckpointFile, CheckpointId, ClientName, CloseReason, CommandBody,
    CommandResult, Credential, CredentialForm, Decision, DiffExtent, Effort,
    Epoch, Event, EventKind, Feed, Field, FieldKind, FilePatch, HistoryAnchor,
    Home, HostPath, HostSeq, InstanceId, InstanceStatus, Instances,
    ModelChoice, ModelId, Models, Provider, ProviderInfo, Question,
    QuestionOption, Rejection, RequestId, Resolution, Selection, SessionId,
    SignIn, SignInCode, SignInStep, SubagentActivity, SubagentEvent,
    SubagentId, SubagentOutcome, Ticket, TicketError, ToolCallId, ToolKind,
    ToolOutcome, ToolPhase, TurnOutcome, UndoAvailability, Welcome,
};

use crate::{
    Action, AgentGroup, Ask, CredentialIntent, Detail, Diff, DiffFile,
    DiffPanel, Effect, EffortControl, Focus, Footer, FormIntent, Hint, HostId,
    HostsScreen, Intent, Keystroke, Listing, Mount, NewSessionForm, Opening,
    Patch, Place, Preview, QuestionBody, Row, Saved, Screen, SessionIntent,
    SessionPane, State, Stored, ToolDetail, ToolLine, TurnChanges, UndoNote,
    sends, update, view,
};

const SESSION: SessionId = SessionId(7);
const HOST: HostId = HostId([9; 32]);

fn text(text: &str) -> String {
    String::from(text)
}

fn ticket() -> Ticket {
    Ticket {
        host: [9; 32],
        relay: text("https://relay.example/"),
        token: AuthToken::from_bytes([1; 16]),
    }
}

/// The app opened on a join link, with nothing saved.
fn joined(link: Result<Ticket, TicketError>) -> State {
    State::new(Opening {
        stored: Stored::default(),
        joined: Some(link),
    })
}

/// Paired with a host whose catalog arrived but which isn't live.
fn paired() -> State {
    let (state, _) = update(
        joined(Ok(ticket())),
        welcomed(HOST, "desk", Instances::default(), None),
    );
    state
}

/// The providers as a host's plugins describe them.
fn catalog() -> Catalog {
    let field = |id: &str, label: &str, kind| Field {
        id: text(id),
        label: text(label),
        kind,
        placeholder: String::new(),
        required: true,
    };
    let form = |kind: &str, label: &str, fields| CredentialForm {
        kind: text(kind),
        label: text(label),
        fields,
        account: text("base_url"),
    };
    Catalog(BTreeMap::from([
        (
            Provider::Claude,
            ProviderInfo {
                label: text("Claude Code"),
                note: None,
                credentials: Vec::from([form(
                    "gateway",
                    "Gateway",
                    Vec::from([
                        field("base_url", "Gateway URL", FieldKind::Url),
                        field("token", "Token", FieldKind::Secret),
                    ]),
                )]),
            },
        ),
        (
            Provider::Codex,
            ProviderInfo {
                label: text("Codex"),
                note: Some(text("Codex runs commands without asking.")),
                credentials: Vec::from([form(
                    "provider",
                    "Model provider",
                    Vec::from([
                        field("name", "Provider name", FieldKind::Text),
                        field("base_url", "Base URL", FieldKind::Url),
                        field("model", "Model", FieldKind::Text),
                        field("key", "Key", FieldKind::Secret),
                    ]),
                )]),
            },
        ),
    ]))
}

/// Applies each intent in turn, and gathers the effects.
fn run(state: State, intents: Vec<Intent>) -> (State, Vec<Effect>) {
    intents.into_iter().fold(
        (state, Vec::new()),
        |(current, mut all), intent| {
            let (next, effects) = update(current, intent);
            all.extend(effects);
            (next, all)
        },
    )
}

fn event(seq: u64, kind: EventKind) -> Intent {
    Intent::Link(
        HOST,
        Update::Feed(Feed::Event(Event {
            seq: HostSeq(seq),
            session: SESSION,
            kind,
        })),
    )
}

fn created() -> EventKind {
    EventKind::SessionCreated {
        instance: id("claude"),
        provider: Provider::Claude,
        directory: HostPath(text("/home/me/conductor")),
        by: ClientName(text("desk")),
        model: None,
        effort: None,
    }
}

fn said(words: &str) -> EventKind {
    EventKind::UserMessage {
        text: text(words),
        by: ClientName(text("phone")),
    }
}

fn welcomed(
    host: HostId,
    name: &str,
    instances: Instances,
    home: Option<Home>,
) -> Intent {
    Intent::Link(
        host,
        Update::Welcome(Welcome {
            host: text(name),
            epoch: Epoch(1),
            head_seq: HostSeq(0),
            instances,
            catalog: catalog(),
            home,
        }),
    )
}

fn connection(host: HostId, state: LinkState, error: Option<&str>) -> Intent {
    Intent::Link(
        host,
        Update::Status(Status {
            state,
            error: error.map(text),
        }),
    )
}

fn roster(instances: Instances) -> Intent {
    Intent::Link(HOST, Update::Feed(Feed::Instances(instances)))
}

/// A provider's own instance by its name, and otherwise Claude's of the
/// account.
fn id(name: &str) -> InstanceId {
    match name {
        "claude" => InstanceId::own(Provider::Claude),
        "codex" => InstanceId::own(Provider::Codex),
        account => InstanceId::named(Provider::Claude, account),
    }
}

const fn instance(availability: Availability) -> InstanceStatus {
    InstanceStatus { availability }
}

/// A host with only its default Claude instance.
fn claude(availability: Availability) -> Instances {
    Instances(BTreeMap::from([(id("claude"), instance(availability))]))
}

/// A session that is ready, shown in the detail pane.
fn opened(more: Vec<Intent>) -> (State, Vec<Effect>) {
    let shown = Vec::from([
        event(1, created()),
        event(2, EventKind::SessionReady),
        Intent::Show(Place::Session(HOST, SESSION)),
    ]);
    run(paired(), shown.into_iter().chain(more).collect())
}

fn screen(state: &State) -> Option<HostsScreen> {
    match view(state) {
        Screen::Hosts(host) => Some(*host),
        Screen::Pair(_) => None,
    }
}

fn pane(state: &State) -> Option<SessionPane> {
    screen(state).and_then(|host| match host.detail {
        Detail::Session(pane) => Some(*pane),
        Detail::Placeholder(_)
        | Detail::Pair(_)
        | Detail::NewSession(_)
        | Detail::Credentials(_)
        | Detail::Projects(_)
        | Detail::Issue(_) => None,
    })
}

/// The first host's sessions.
fn listing(state: &State) -> Option<Listing> {
    screen(state).and_then(|host| {
        host.list
            .groups
            .into_iter()
            .next()
            .map(|group| group.listing)
    })
}

fn rows(state: &State) -> Vec<Row> {
    pane(state).map_or_else(Vec::new, |shown| {
        shown.entries.into_iter().map(|entry| entry.row).collect()
    })
}

fn form(state: &State) -> Option<NewSessionForm> {
    screen(state).and_then(|host| match host.detail {
        Detail::NewSession(form) => Some(*form),
        Detail::Placeholder(_)
        | Detail::Pair(_)
        | Detail::Credentials(_)
        | Detail::Projects(_)
        | Detail::Session(_)
        | Detail::Issue(_) => None,
    })
}

fn sent(effects: &[Effect]) -> Vec<CommandBody> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Send { body, .. } => Some(body.clone()),
            Effect::PairHost(_)
            | Effect::ForgetHost(_)
            | Effect::NameHost(_)
            | Effect::Focus(_)
            | Effect::ShowEffort(_) => None,
        })
        .collect()
}

fn answer(effects: &[Effect], result: Result<CommandResult, String>) -> Intent {
    let sent = effects.iter().find_map(|effect| match effect {
        Effect::Send { reply, .. } => Some(reply),
        _ => None,
    });
    assert!(sent.is_some(), "expected a command to answer");
    sent.map_or(Intent::DismissNotice, |reply| {
        Intent::Replied(reply.clone(), result)
    })
}

fn local(effects: &[Effect]) -> Vec<Effect> {
    effects
        .iter()
        .filter(|effect| {
            !matches!(
                effect,
                Effect::Send { .. }
                    | Effect::PairHost(_)
                    | Effect::ForgetHost(_)
                    | Effect::NameHost(_)
            )
        })
        .cloned()
        .collect()
}

fn line(state: &'static str, command: &str) -> ToolLine {
    ToolLine {
        state,
        detail: ToolDetail::Command(text(command)),
    }
}

#[test]
fn pairing_saves_a_valid_link_reports_a_bad_one_and_forgets_the_host() {
    let refused = joined(Err(TicketError::Truncated));
    let (typed, _) = run(refused, Vec::from([Intent::TypeJoin(text(" "))]));
    let (paired_now, saved) =
        run(typed.clone(), Vec::from([Intent::Join(Ok(ticket()))]));
    let (forgot, forgotten) =
        run(paired_now.clone(), Vec::from([Intent::Forget(HOST)]));
    let pair = |state: &State| match view(state) {
        Screen::Pair(pair) => Some((pair.connect_disabled, pair.error)),
        Screen::Hosts(_) => None,
    };
    let invalid =
        Some(text("That join link is not valid: ticket is truncated"));
    assert_eq!(
        (
            pair(&typed),
            screen(&paired_now).map(|host| host.links),
            saved,
            pair(&forgot),
            forgotten,
        ),
        (
            Some((true, invalid.clone())),
            Some(Vec::from([crate::Dial {
                host: HOST,
                key: ticket().to_text(),
                ticket: ticket(),
            }])),
            Vec::from([Effect::PairHost(Saved {
                ticket: ticket(),
                name: None,
            })]),
            Some((true, invalid)),
            Vec::from([Effect::ForgetHost(HOST)]),
        )
    );
}

#[test]
fn a_phone_shows_the_list_until_a_pane_is_chosen() {
    let (shown, _) = opened(Vec::new());
    let place = |state: State, place: Place| {
        let (next, _) = run(state, Vec::from([Intent::Show(place)]));
        screen(&next).map(|host| host.shown)
    };
    let (reset, _) = run(
        shown.clone(),
        Vec::from([Intent::Link(HOST, Update::Reset(proto::Epoch(2)))]),
    );
    assert_eq!(
        [
            screen(&paired()).map(|host| host.shown),
            screen(&shown).map(|host| host.shown),
            place(shown.clone(), Place::NewSession),
            place(shown.clone(), Place::Credentials),
            place(shown, Place::Sessions),
            screen(&reset).map(|host| host.shown),
        ],
        [
            Some("list"),
            Some("detail"),
            Some("detail"),
            Some("detail"),
            Some("list"),
            Some("list"),
        ]
    );
}

#[test]
fn the_list_marks_the_shown_session_and_what_waits_on_a_person() {
    let (state, _) = opened(Vec::from([
        event(3, said("go")),
        event(
            4,
            EventKind::RequestOpened {
                request: RequestId(1),
                call: ToolCallId(text("toolu_1")),
                tool: ToolKind::Command {
                    command: text("ls"),
                },
                reason: None,
                for_session: None,
            },
        ),
    ]));
    let row = match listing(&state) {
        Some(Listing::Rows(rows)) => rows.into_iter().next().map(|row| {
            (row.status, row.current, row.title, row.sub, row.badge)
        }),
        Some(Listing::Empty(_)) | None => None,
    };
    assert_eq!(
        (row, listing(&paired())),
        (
            Some((
                text("approval"),
                true,
                text("conductor"),
                text("Claude Code, needs you"),
                text("1")
            )),
            Some(Listing::Empty(text("No sessions on desk yet."))),
        )
    );
}

#[test]
fn every_transcript_item_has_its_row() {
    let call = ToolCallId(text("toolu_1"));
    let (state, _) = opened(Vec::from([
        event(3, said("hi")),
        event(4, EventKind::Reasoning { text: text("plan") }),
        event(
            5,
            EventKind::AssistantMessage {
                text: text("Hello."),
            },
        ),
        event(
            6,
            EventKind::ToolCall {
                call: call.clone(),
                phase: ToolPhase::Started {
                    tool: ToolKind::FileChange { files: Vec::new() },
                },
            },
        ),
        event(
            7,
            EventKind::ToolCall {
                call,
                phase: ToolPhase::Finished {
                    outcome: ToolOutcome::Succeeded,
                },
            },
        ),
        event(
            8,
            EventKind::ModelChanged {
                model: None,
                effort: None,
                reason: text("switched"),
            },
        ),
        event(
            9,
            EventKind::Error {
                message: text("oops"),
            },
        ),
        event(
            10,
            EventKind::TurnCompleted {
                outcome: TurnOutcome::Failed {
                    message: text("boom"),
                },
            },
        ),
        event(
            11,
            EventKind::SessionClosed {
                reason: CloseReason::Exited { status: text("0") },
            },
        ),
        event(12, EventKind::SessionReady),
        event(13, EventKind::ReasoningDelta { text: text("thin") }),
    ]));
    let (streaming, _) = run(
        state.clone(),
        Vec::from([event(14, EventKind::AssistantDelta { text: text("He") })]),
    );
    assert_eq!(
        (rows(&state), rows(&streaming).pop()),
        (
            Vec::from([
                Row::User {
                    text: text("hi"),
                    by: text("phone")
                },
                Row::Thought(text("plan")),
                Row::Reply {
                    class: "reply",
                    text: text("Hello.")
                },
                Row::Tool(ToolLine {
                    state: "succeeded",
                    detail: ToolDetail::NoFiles
                }),
                Row::ModelChanged(text("switched")),
                Row::Error(text("oops")),
                Row::TurnEnd {
                    outcome: "failed",
                    text: text("Failed: boom")
                },
                Row::Continued(text(
                    "Continued after: The session has ended (0)."
                )),
                Row::Thinking(text("thin")),
            ]),
            Some(Row::Reply {
                class: "reply streaming",
                text: text("He")
            }),
        )
    );
}

fn agent_call(call: &str, description: &str) -> EventKind {
    EventKind::ToolCall {
        call: ToolCallId(text(call)),
        phase: ToolPhase::Started {
            tool: ToolKind::Other {
                name: text("Agent"),
                detail: text(description),
            },
        },
    }
}

fn agent_started(agent: &str, call: &str, description: &str) -> EventKind {
    EventKind::Subagent {
        agent: SubagentId(text(agent)),
        event: SubagentEvent::Started {
            call: ToolCallId(text(call)),
            description: text(description),
        },
    }
}

#[test]
fn each_subagent_hangs_under_the_call_that_started_it() {
    let (state, _) = opened(Vec::from([
        event(3, said("look around")),
        event(4, agent_call("toolu_a", "Find the docs")),
        event(5, agent_started("a", "toolu_a", "Find the docs")),
        event(6, agent_call("toolu_b", "Run the tests")),
        event(7, agent_started("b", "toolu_b", "Run the tests")),
        event(
            8,
            EventKind::Subagent {
                agent: SubagentId(text("a")),
                event: SubagentEvent::Activity(SubagentActivity::ToolCall {
                    call: ToolCallId(text("toolu_a1")),
                    phase: ToolPhase::Started {
                        tool: ToolKind::Command {
                            command: text("ls docs"),
                        },
                    },
                }),
            },
        ),
        event(
            9,
            EventKind::Subagent {
                agent: SubagentId(text("a")),
                event: SubagentEvent::Ended {
                    outcome: SubagentOutcome::Failed,
                    summary: Some(text("no docs")),
                },
            },
        ),
    ]));
    let groups = pane(&state).map(|shown| {
        shown
            .entries
            .into_iter()
            .map(|entry| entry.agents)
            .collect::<Vec<Vec<AgentGroup>>>()
    });
    assert_eq!(
        groups,
        Some(Vec::from([
            Vec::new(),
            Vec::from([AgentGroup {
                key: text("a"),
                description: text("Find the docs"),
                status: "failed",
                label: "Failed",
                rows: Vec::from([
                    Row::Tool(line("stopped", "ls docs")),
                    Row::Reply {
                        class: "reply",
                        text: text("no docs"),
                    },
                ]),
                empty: None,
            }]),
            Vec::from([AgentGroup {
                key: text("b"),
                description: text("Run the tests"),
                status: "running",
                label: "Running",
                rows: Vec::new(),
                empty: Some("Nothing yet"),
            }]),
        ]))
    );
}

fn asked(for_session: Option<String>) -> Intent {
    event(
        4,
        EventKind::RequestOpened {
            request: RequestId(3),
            call: ToolCallId(text("toolu_3")),
            tool: ToolKind::Command {
                command: text("rm a"),
            },
            reason: Some(text("to clean")),
            for_session,
        },
    )
}

fn card(state: &State) -> Option<crate::ApprovalCard> {
    rows(state).into_iter().find_map(|row| match row {
        Row::Approval(card) => Some(card),
        _ => None,
    })
}

#[test]
fn an_open_approval_offers_its_decisions_and_resolves_in_place() {
    let (open, decided) = opened(Vec::from([
        event(3, said("go")),
        asked(Some(text("rm"))),
        Intent::Session(SessionIntent::Decide(
            RequestId(3),
            Decision::ApproveForSession,
        )),
    ]));
    let (resolved, _) = run(
        open.clone(),
        Vec::from([event(
            5,
            EventKind::RequestResolved {
                request: RequestId(3),
                resolution: Resolution::Decided {
                    decision: Decision::ApproveForSession,
                    by: ClientName(text("phone")),
                },
            },
        )]),
    );
    let shown =
        |state: &State| card(state).map(|card| (card.id, card.open, card.ask));
    assert_eq!(
        (
            shown(&open),
            shown(&resolved),
            sent(&decided),
            local(&decided)
        ),
        (
            Some((
                text("request-3"),
                true,
                Ask::Open {
                    actions: Vec::from([
                        Action {
                            class: "deny",
                            label: "Deny",
                            decision: Decision::Deny
                        },
                        Action {
                            class: "approve",
                            label: "Approve",
                            decision: Decision::Approve
                        },
                    ]),
                    for_session: Some(text("rm")),
                }
            )),
            Some((
                text("request-3"),
                false,
                Ask::Resolved(text("Allowed for this session by phone"))
            )),
            Vec::from([CommandBody::Resolve {
                session: SESSION,
                request: RequestId(3),
                decision: Decision::ApproveForSession,
            }]),
            Vec::from([Effect::Focus(Focus::Element(text("request-3")))]),
        )
    );
}

#[test]
fn a_request_left_open_when_the_turn_ends_is_withdrawn() {
    let (running, _) = opened(Vec::from([event(3, said("go")), asked(None)]));
    let (ended, _) = run(
        running.clone(),
        Vec::from([event(
            5,
            EventKind::TurnCompleted {
                outcome: TurnOutcome::Interrupted,
            },
        )]),
    );
    assert_eq!(
        (
            card(&running).map(|card| card.tool),
            card(&ended).map(|card| (card.tool, card.reason, card.ask)),
        ),
        (
            Some(line("running", "rm a")),
            Some((
                line("stopped", "rm a"),
                Some(text("to clean")),
                Ask::Resolved(text("Withdrawn"))
            )),
        )
    );
}

fn questions() -> Vec<Question> {
    let option = |label: &str| QuestionOption {
        label: text(label),
        description: text("a colour"),
    };
    Vec::from([
        Question {
            header: text("Colour"),
            text: text("Which?"),
            options: Vec::from([option("Red"), option("Blue")]),
            selection: Selection::One,
        },
        Question {
            header: text("Extras"),
            text: text("Any?"),
            options: Vec::from([option("Nuts")]),
            selection: Selection::Many,
        },
    ])
}

fn question_body(state: &State) -> Option<QuestionBody> {
    rows(state).into_iter().find_map(|row| match row {
        Row::Question(card) => Some(card.body),
        _ => None,
    })
}

#[test]
fn a_question_form_sends_once_every_question_has_an_answer() {
    let request = RequestId(4);
    let (open, early) = opened(Vec::from([
        event(3, said("go")),
        event(
            4,
            EventKind::QuestionAsked {
                request,
                questions: questions(),
            },
        ),
        Intent::Session(SessionIntent::Choose {
            request,
            question: 0,
            label: text("Blue"),
        }),
        Intent::Session(SessionIntent::SendAnswer(request)),
    ]));
    let (answered, answers) = run(
        open.clone(),
        Vec::from([
            Intent::Session(SessionIntent::TypeAnswer {
                request,
                question: 1,
                text: text(" Plum "),
            }),
            Intent::Session(SessionIntent::SendAnswer(request)),
        ]),
    );
    let fields = |state: &State| match question_body(state) {
        Some(QuestionBody::Form(form)) => Some((
            form.send_disabled,
            form.fields
                .into_iter()
                .map(|field| {
                    (
                        field.hint,
                        field
                            .options
                            .into_iter()
                            .map(|option| option.pressed)
                            .collect(),
                        field.typed,
                    )
                })
                .collect::<Vec<(&str, Vec<bool>, String)>>(),
        )),
        Some(QuestionBody::Answered { .. }) | None => None,
    };
    assert_eq!(
        (
            fields(&open),
            early,
            fields(&answered).map(|(off, _)| off),
            sent(&answers)
        ),
        (
            Some((
                true,
                Vec::from([
                    (
                        "Choose one, or write your own",
                        Vec::from([false, true]),
                        String::new()
                    ),
                    (
                        "Choose any, and add your own",
                        Vec::from([false]),
                        String::new()
                    ),
                ])
            )),
            Vec::new(),
            Some(false),
            Vec::from([CommandBody::Answer {
                session: SESSION,
                request,
                answers: Vec::from([
                    proto::Answer(Vec::from([text("Blue")])),
                    proto::Answer(Vec::from([text("Plum")])),
                ]),
            }]),
        )
    );
}

#[test]
fn an_answered_question_shows_each_answer_and_who_gave_it() {
    let request = RequestId(4);
    let (state, _) = opened(Vec::from([
        event(3, said("go")),
        event(
            4,
            EventKind::QuestionAsked {
                request,
                questions: questions(),
            },
        ),
        event(
            5,
            EventKind::RequestResolved {
                request,
                resolution: Resolution::Answered {
                    answers: Vec::from([proto::Answer(Vec::from([
                        text("Red"),
                        text("Pink"),
                    ]))]),
                    by: ClientName(text("desk")),
                },
            },
        ),
    ]));
    let answered = match question_body(&state) {
        Some(QuestionBody::Answered {
            questions,
            resolved,
        }) => Some((
            questions
                .into_iter()
                .map(|question| (question.text, question.answer))
                .collect::<Vec<(String, Option<String>)>>(),
            resolved,
        )),
        Some(QuestionBody::Form(_)) | None => None,
    };
    assert_eq!(
        answered,
        Some((
            Vec::from([
                (text("Which?"), Some(text("Red, Pink"))),
                (text("Any?"), None),
            ]),
            text("Answered by desk"),
        ))
    );
}

fn composer(state: &State) -> Option<(String, String, bool)> {
    pane(state).map(|shown| match shown.footer {
        Footer::Composer(composer) => {
            (composer.text, composer.placeholder, composer.disabled)
        }
        Footer::Closed { text, .. } => (text, String::new(), true),
    })
}

#[test]
fn the_composer_sends_trimmed_text_only_when_the_agent_takes_messages() {
    let send = Intent::Session(SessionIntent::SendMessage);
    let typed =
        |words: &str| Intent::Session(SessionIntent::TypeMessage(text(words)));
    let (idle, once) = opened(Vec::from([
        typed("  "),
        send.clone(),
        typed(" hi "),
        send.clone(),
    ]));
    let (busy, none) = run(
        idle.clone(),
        Vec::from([event(3, said("hi")), typed("more"), send]),
    );
    assert_eq!(
        (
            sent(&once),
            composer(&idle),
            sent(&none),
            composer(&busy),
            pane(&busy).map(|shown| (
                shown.interrupt,
                shown.working,
                shown.pill
            )),
        ),
        (
            Vec::from([CommandBody::SendMessage {
                session: SESSION,
                text: text("hi")
            }]),
            Some((String::new(), text("Message Claude Code"), false)),
            Vec::new(),
            Some((text("more"), text("Claude Code is working…"), true)),
            Some((true, true, text("Claude Code, running"))),
        )
    );
}

#[test]
fn a_failed_send_keeps_the_text_typed_while_the_message_was_pending() {
    let send = Intent::Session(SessionIntent::SendMessage);
    let typed =
        |words: &str| Intent::Session(SessionIntent::TypeMessage(text(words)));
    let (pending, sent_message) =
        opened(Vec::from([typed(" first "), send.clone()]));
    let (rejected, blocked) = run(
        pending,
        Vec::from([
            typed("next"),
            send,
            event(3, said("another client's turn")),
            answer(
                &sent_message,
                Ok(CommandResult::Rejected(Rejection::TurnInProgress)),
            ),
        ]),
    );
    assert_eq!(sent(&blocked), []);
    assert_eq!(composer(&rejected).map(|shown| shown.0), Some(text("next")));
    assert_eq!(
        pane(&rejected).and_then(|shown| match shown.footer {
            Footer::Composer(composer) => Some(composer.refused),
            Footer::Closed { .. } => None,
        }),
        Some(Vec::from([text(" first ")]))
    );

    let (retried, retried_message) = run(
        rejected,
        Vec::from([
            event(
                4,
                EventKind::TurnCompleted {
                    outcome: TurnOutcome::Completed,
                },
            ),
            Intent::Session(SessionIntent::RetryMessage),
        ]),
    );
    assert_eq!(
        sent(&retried_message),
        [CommandBody::SendMessage {
            session: SESSION,
            text: text("first")
        }]
    );
    let (accepted, _) = run(
        retried,
        Vec::from([answer(&retried_message, Ok(CommandResult::Accepted))]),
    );
    assert_eq!(composer(&accepted).map(|shown| shown.0), Some(text("next")));
}

#[test]
fn a_failed_transport_restores_the_submitted_text_to_an_empty_composer() {
    let send = Intent::Session(SessionIntent::SendMessage);
    let (pending, sent_message) = opened(Vec::from([
        Intent::Session(SessionIntent::TypeMessage(text(" first "))),
        send.clone(),
    ]));
    let (restored, _) = run(
        pending,
        Vec::from([answer(&sent_message, Err(text("connection lost")))]),
    );
    assert_eq!(
        composer(&restored).map(|shown| shown.0),
        Some(text(" first "))
    );
    let (_, retry) = run(restored, Vec::from([send]));
    assert_eq!(sent(&retry), sent(&sent_message));
}

#[test]
fn return_sends_unless_shifted_or_composing() {
    let key = |enter, shift, composing| Keystroke {
        enter,
        shift,
        composing,
    };
    assert_eq!(
        [
            key(true, false, false),
            key(true, true, false),
            key(true, false, true),
            key(false, false, false),
        ]
        .map(sends),
        [true, false, false, false]
    );
}

#[test]
fn a_closed_session_offers_continue_and_focuses_the_composer_that_follows() {
    let (closed, _) = opened(Vec::from([event(
        3,
        EventKind::SessionClosed {
            reason: CloseReason::Stopped {
                reason: text("reset"),
            },
        },
    )]));
    let (continued, effects) = run(
        closed.clone(),
        Vec::from([
            Intent::Session(SessionIntent::Continue),
            event(4, EventKind::SessionReady),
            Intent::Session(SessionIntent::Mounted(Mount::Composer)),
            Intent::Session(SessionIntent::Mounted(Mount::Composer)),
        ]),
    );
    assert_eq!(
        (
            pane(&closed).map(|shown| shown.footer),
            sent(&effects),
            local(&effects),
            pane(&continued).map(|shown| shown.status),
        ),
        (
            Some(Footer::Closed {
                text: text("Stopped: reset"),
                resume: true
            }),
            Vec::from([CommandBody::Continue { session: SESSION }]),
            Vec::from([Effect::Focus(Focus::Composer)]),
            Some(text("idle")),
        )
    );
}

#[test]
fn removing_a_closed_session_sends_the_removal_and_leaves_its_pane() {
    let (closed, _) = opened(Vec::from([event(
        3,
        EventKind::SessionClosed {
            reason: CloseReason::Stopped {
                reason: text("reset"),
            },
        },
    )]));
    let (removed, effects) = run(
        closed,
        Vec::from([
            Intent::Session(SessionIntent::Remove),
            event(4, EventKind::SessionRemoved { kept: None }),
        ]),
    );
    assert_eq!(
        (sent(&effects), pane(&removed)),
        (
            Vec::from([CommandBody::RemoveSession { session: SESSION }]),
            None,
        )
    );
}

fn models() -> Models {
    Models {
        choices: Vec::from([ModelChoice {
            id: ModelId(text("opus")),
            name: text("Opus"),
            description: text("best"),
            efforts: Vec::from([Effort::Low, Effort::High]),
        }]),
        default: Some(ModelId(text("opus"))),
    }
}

fn signed_in() -> Availability {
    Availability::Available {
        auth: Auth::SignedIn,
        models: models(),
    }
}

#[test]
fn the_effort_picker_offers_the_models_efforts_and_puts_back_a_refusal() {
    let (state, effects) = opened(Vec::from([
        connection(HOST, LinkState::Live, None),
        roster(claude(signed_in())),
        Intent::Session(SessionIntent::PickEffort(text("high"))),
        Intent::Session(SessionIntent::PickEffort(String::new())),
    ]));
    let (refused, put_back) = run(
        state.clone(),
        Vec::from([answer(
            &effects,
            Ok(CommandResult::Rejected(Rejection::TurnInProgress)),
        )]),
    );
    let (returned, _) = run(
        state.clone(),
        Vec::from([
            Intent::Show(Place::Sessions),
            Intent::Show(Place::Session(HOST, SESSION)),
        ]),
    );
    let (late, ignored) = run(
        returned.clone(),
        Vec::from([answer(
            &effects,
            Ok(CommandResult::Rejected(Rejection::TurnInProgress)),
        )]),
    );
    assert_eq!(pane(&late), pane(&returned));
    assert_eq!(ignored, []);
    let picker = pane(&state).map(|shown| match shown.effort {
        EffortControl::Picker(choices) => choices
            .into_iter()
            .map(|choice| {
                (choice.value, choice.label, choice.selected, choice.disabled)
            })
            .collect(),
        EffortControl::Hidden | EffortControl::Fixed(_) => Vec::new(),
    });
    assert_eq!(
        (
            picker,
            sent(&effects),
            local(&effects),
            put_back,
            pane(&refused).and_then(|shown| shown.notice),
        ),
        (
            Some(Vec::from([
                (String::new(), text("default effort"), true, true),
                (text("low"), text("low effort"), false, false),
                (text("high"), text("high effort"), false, false),
            ])),
            Vec::from([CommandBody::SetEffort {
                session: SESSION,
                effort: Effort::High,
            }]),
            Vec::new(),
            Vec::from([Effect::ShowEffort(String::new())]),
            Some(text("a turn is already running")),
        )
    );
}

#[test]
fn the_new_session_form_validates_and_shows_refusals_where_they_belong() {
    let instances = Instances(BTreeMap::from([
        (id("claude"), instance(signed_in())),
        (
            id("codex"),
            instance(Availability::Unavailable {
                reason: text("no codex"),
            }),
        ),
    ]));
    let (empty, _) = run(
        paired(),
        Vec::from([
            connection(HOST, LinkState::Live, None),
            roster(instances),
            Intent::Show(Place::NewSession),
        ]),
    );
    let (codex, _) = run(
        empty.clone(),
        Vec::from([Intent::Form(FormIntent::PickInstance(id("codex")))]),
    );
    let (starting, started) = run(
        empty.clone(),
        Vec::from([
            Intent::Form(FormIntent::PickModel(text("opus"))),
            Intent::Form(FormIntent::PickEffort(text("high"))),
            Intent::Form(FormIntent::TypeDirectory(text(" /src/x "))),
            Intent::Form(FormIntent::Start),
        ]),
    );
    let answered = |result| {
        run(starting.clone(), Vec::from([answer(&started, Ok(result))])).0
    };
    let refused = answered(CommandResult::Rejected(Rejection::NoSuchDirectory));
    let button = |state: &State| {
        form(state).map(|shown| (shown.start_disabled, shown.start_label))
    };
    assert_eq!(
        (
            button(&empty),
            form(&empty).map(|shown| (
                shown.models.len(),
                shown.efforts.len(),
                shown.suggestions
            )),
            form(&codex).and_then(|shown| shown.instance_hint),
            button(&starting),
            sent(&started),
            form(&refused)
                .map(|shown| (shown.directory_invalid, shown.directory_error)),
            button(&refused),
            screen(
                &run(
                    answered(CommandResult::Started { session: SESSION }),
                    Vec::from([event(1, created())]),
                )
                .0
            )
            .map(|host| host.shown),
        ),
        (
            Some((true, "Start session")),
            Some((2, 3, None)),
            Some(Hint {
                class: "hint error",
                role: Some("alert"),
                text: text("Codex can't run on desk: no codex"),
            }),
            Some((true, "Starting…")),
            Vec::from([CommandBody::StartSession {
                instance: id("claude"),
                at: StartAt::Directory(HostPath(text("/src/x"))),
                model: Some(ModelId(text("opus"))),
                effort: Some(Effort::High),
            }]),
            Some((
                true,
                Some(Hint {
                    class: "hint error",
                    role: Some("alert"),
                    text: text("There is no directory at that path on desk."),
                })
            )),
            Some((false, "Start session")),
            Some("detail"),
        )
    );
}

type Shown = (String, Option<String>);

fn credentials(state: &State) -> Vec<Shown> {
    pane_of(state).map_or_else(Vec::new, |pane| {
        pane.instances
            .into_iter()
            .map(|shown| (shown.state, shown.outcome.map(|hint| hint.text)))
            .collect()
    })
}

/// Claude's forms: whether saving is off, what came of the last, and each
/// field's value.
fn claude_forms(state: &State) -> Option<(bool, Option<String>, Vec<String>)> {
    pane_of(state).and_then(|pane| {
        pane.add
            .into_iter()
            .find(|forms| forms.provider == Provider::Claude)
            .map(|forms| {
                (
                    forms.forms.iter().any(|form| form.save_disabled),
                    forms.outcome.map(|hint| hint.text),
                    forms
                        .forms
                        .into_iter()
                        .flat_map(|form| form.fields)
                        .map(|field| field.value)
                        .collect(),
                )
            })
    })
}

#[test]
fn a_secret_is_sent_once_and_its_account_makes_the_instance() {
    let fill = |field: &str, value: &str| {
        Intent::Credential(
            HOST,
            CredentialIntent::Type {
                provider: Provider::Claude,
                kind: text("gateway"),
                field: text(field),
                text: text(value),
            },
        )
    };
    let (filled, _) = run(
        paired(),
        Vec::from([
            connection(HOST, LinkState::Live, None),
            roster(claude(signed_in())),
            Intent::Show(Place::Credentials),
            fill("base_url", " https://gw.example "),
            fill("token", "secret"),
        ]),
    );
    let (saving, saved) = run(
        filled.clone(),
        Vec::from([Intent::Credential(
            HOST,
            CredentialIntent::Save {
                provider: Provider::Claude,
                kind: text("gateway"),
            },
        )]),
    );
    let (stored, _) = run(
        saving.clone(),
        Vec::from([
            answer(&saved, Ok(CommandResult::Instances(two_claudes()))),
            connection(HOST, LinkState::Live, None),
            roster(two_claudes()),
        ]),
    );
    assert_eq!(
        (
            claude_forms(&filled).map(|forms| forms.0),
            claude_forms(&saving),
            sent(&saved),
            claude_forms(&stored).map(|forms| forms.1),
            credentials(&stored),
        ),
        (
            Some(false),
            Some((
                true,
                Some(text("Sending…")),
                Vec::from([text(" https://gw.example "), String::new()]),
            )),
            Vec::from([CommandBody::SetCredential {
                provider: Provider::Claude,
                credential: Credential {
                    kind: text("gateway"),
                    values: BTreeMap::from([
                        (text("base_url"), text("https://gw.example")),
                        (text("token"), text("secret")),
                    ]),
                },
            }]),
            Some(Some(text(
                "Sent to desk over the encrypted link. This app doesn't keep it; your browser may offer to save it."
            ))),
            Vec::from([
                (text("Signed in on desk"), None),
                (text("Gateway at https://gw.example"), None),
            ]),
        )
    );
}

/// A host with Claude's own instance, signed in, and the instance of a
/// gateway's account.
fn two_claudes() -> Instances {
    Instances(BTreeMap::from([
        (id("claude"), instance(signed_in())),
        (
            id("https://gw.example"),
            instance(Availability::Available {
                auth: Auth::Stored {
                    kind: text("gateway"),
                    url: Some(text("https://gw.example")),
                },
                models: Models::none(),
            }),
        ),
    ]))
}

/// The first host's credentials.
fn pane_of(state: &State) -> Option<crate::CredentialsPane> {
    screen(state).and_then(|host| match host.detail {
        Detail::Credentials(shown) => shown.hosts.into_iter().next(),
        Detail::Placeholder(_)
        | Detail::Pair(_)
        | Detail::NewSession(_)
        | Detail::Projects(_)
        | Detail::Session(_)
        | Detail::Issue(_) => None,
    })
}

#[test]
fn each_instance_is_its_account_and_each_provider_has_its_forms() {
    let (shown, _) = run(
        paired(),
        Vec::from([
            connection(HOST, LinkState::Live, None),
            roster(two_claudes()),
            Intent::Show(Place::Credentials),
        ]),
    );
    let sections = pane_of(&shown).map(|pane| {
        (
            pane.instances
                .iter()
                .map(|it| {
                    (
                        it.heading.clone(),
                        it.name.clone(),
                        it.provider.clone(),
                        it.state.clone(),
                        it.remove.is_some(),
                        it.login.clone(),
                    )
                })
                .collect::<Vec<_>>(),
            pane.add
                .iter()
                .map(|forms| (forms.label.clone(), forms.forms.len()))
                .collect::<Vec<_>>(),
        )
    });
    assert_eq!(
        sections,
        Some((
            Vec::from([
                (
                    format!("{}-claude-own-credential", HOST.key()),
                    text("Claude Code"),
                    text("Claude Code"),
                    text("Signed in on desk"),
                    false,
                    text(
                        "To sign in on desk's terminal: conductord login claude"
                    ),
                ),
                (
                    format!(
                        "{}-claude-https://gw.example-credential",
                        HOST.key()
                    ),
                    text("https://gw.example"),
                    text("Claude Code"),
                    text("Gateway at https://gw.example"),
                    true,
                    text(
                        "To sign it in again on desk's terminal: conductord login claude --account='https://gw.example'"
                    ),
                ),
            ]),
            Vec::from([(text("Claude Code"), 1), (text("Codex"), 1)]),
        ))
    );
}

#[test]
fn an_instance_is_removed_and_a_refusal_shows_why() {
    let (shown, _) = run(
        paired(),
        Vec::from([
            connection(HOST, LinkState::Live, None),
            roster(two_claudes()),
            Intent::Show(Place::Credentials),
        ]),
    );
    let api = || id("https://gw.example");
    let (removing, removed) = run(
        shown,
        Vec::from([Intent::Credential(HOST, CredentialIntent::Remove(api()))]),
    );
    let (refused, _) = run(
        removing,
        Vec::from([answer(
            &removed,
            Ok(CommandResult::Rejected(Rejection::InstanceInUse {
                sessions: 1,
            })),
        )]),
    );
    let outcome = |state: &State| {
        pane_of(state).and_then(|pane| {
            pane.instances
                .into_iter()
                .find(|it| it.instance == api())
                .and_then(|it| it.outcome)
                .map(|hint| hint.text)
        })
    };
    assert_eq!(
        (sent(&removed), removed.last().cloned(), outcome(&refused)),
        (
            Vec::from([CommandBody::RemoveInstance { instance: api() }]),
            Some(Effect::Focus(Focus::Element(format!(
                "{}-add-instance",
                HOST.key()
            )))),
            Some(text(
                "a session is running on it; stop it before removing the instance"
            )),
        )
    );
}

#[test]
fn the_new_session_form_lists_each_instance_and_starts_on_the_one_picked() {
    let (shown, _) = run(
        paired(),
        Vec::from([
            connection(HOST, LinkState::Live, None),
            roster(two_claudes()),
            Intent::Show(Place::NewSession),
        ]),
    );
    let (_, started) = run(
        shown.clone(),
        Vec::from([
            Intent::Form(FormIntent::PickInstance(id("https://gw.example"))),
            Intent::Form(FormIntent::TypeDirectory(text("/src/x"))),
            Intent::Form(FormIntent::Start),
        ]),
    );
    assert_eq!(
        (
            form(&shown).map(|drawn| drawn
                .instances
                .into_iter()
                .map(|choice| (choice.label, choice.checked))
                .collect::<Vec<_>>()),
            sent(&started),
        ),
        (
            Some(Vec::from([
                (text("Claude Code"), true),
                (text("https://gw.example"), false),
            ])),
            Vec::from([CommandBody::StartSession {
                instance: id("https://gw.example"),
                at: StartAt::Directory(HostPath(text("/src/x"))),
                model: None,
                effort: None,
            }]),
        )
    );
}

fn a_turn(undo: UndoAvailability) -> Vec<Intent> {
    Vec::from([
        event(3, said("go")),
        event(
            4,
            EventKind::TurnCompleted {
                outcome: TurnOutcome::Completed,
            },
        ),
        event(
            5,
            EventKind::CheckpointCreated {
                checkpoint: Checkpoint {
                    id: CheckpointId(1),
                    parent: Some(CheckpointId(0)),
                    turn_start: Some(HostSeq(3)),
                    commit: text("abc"),
                    history: HistoryAnchor::Fresh,
                    undo,
                },
            },
        ),
    ])
}

fn changes(state: &State) -> Option<TurnChanges> {
    pane(state).and_then(|shown| {
        shown.entries.into_iter().find_map(|entry| entry.changes)
    })
}

fn diff() -> CheckpointDiff {
    CheckpointDiff {
        checkpoint: CheckpointId(1),
        parent: CheckpointId(0),
        files: Vec::from([CheckpointFile {
            path: text("a.txt"),
            change: proto::Change::Write,
            patch: FilePatch::Binary,
        }]),
        extent: DiffExtent::Truncated,
    }
}

#[test]
fn a_turns_changes_load_once_and_offer_undo_only_while_idle() {
    let show = Intent::Session(SessionIntent::ShowChanges(CheckpointId(1)));
    let turn = |undo| {
        opened(
            a_turn(undo)
                .into_iter()
                .chain([show.clone(), show.clone()])
                .collect(),
        )
    };
    let (loading, asked) = turn(UndoAvailability::Available);
    let loaded_with =
        |result| run(loading.clone(), Vec::from([answer(&asked, result)])).0;
    let loaded = loaded_with(Ok(CommandResult::Diff(diff())));
    let (busy, _) = run(loaded.clone(), Vec::from([event(6, said("again"))]));
    let (not_offered, unavailable_request) =
        turn(UndoAvailability::Unavailable {
            reason: text("no worktree"),
        });
    let unavailable = run(
        not_offered,
        Vec::from([answer(
            &unavailable_request,
            Ok(CommandResult::Diff(diff())),
        )]),
    )
    .0;
    let panel = |state: &State| changes(state).map(|turn| turn.panel);
    let undo = |state: &State| match panel(state) {
        Some(DiffPanel::Loaded(Diff {
            undo_disabled,
            undo_hint,
            ..
        })) => Some((undo_disabled, undo_hint)),
        _ => None,
    };
    assert_eq!(
        (
            changes(&loading).map(|turn| (
                turn.key,
                turn.button,
                turn.button_disabled,
                turn.panel
            )),
            sent(&asked),
            panel(&loaded),
            undo(&busy),
            undo(&unavailable),
            panel(&loaded_with(Err(text("offline")))),
        ),
        (
            Some((
                text("checkpoint-1"),
                text("checkpoint-1-changes"),
                true,
                DiffPanel::Loading
            )),
            Vec::from([CommandBody::GetDiff {
                session: SESSION,
                checkpoint: CheckpointId(1)
            }]),
            Some(DiffPanel::Loaded(Diff {
                preview: Rc::new(Preview {
                    empty: None,
                    files: Vec::from([DiffFile {
                        path: text("Added a.txt"),
                        patch: Patch::Note(text(
                            "Binary file changed. Preview unavailable."
                        )),
                    }]),
                    truncated: Some(
                        "Preview limit reached. Some changes are omitted."
                    ),
                }),
                undo_disabled: false,
                undo_hint: text("Undo is available when the session is idle."),
            })),
            Some((true, text("Undo is available when the session is idle."))),
            Some((true, text("Undo unavailable: no worktree"))),
            Some(DiffPanel::Failed(text("offline"))),
        )
    );
}

#[test]
fn undo_sends_the_checkpoint_and_the_pane_follows_its_progress() {
    let (state, effects) = opened(
        a_turn(UndoAvailability::Available)
            .into_iter()
            .chain([Intent::Session(SessionIntent::Undo(CheckpointId(1)))])
            .collect(),
    );
    let after = |kind| run(state.clone(), Vec::from([event(6, kind)])).0;
    let undoing = after(EventKind::UndoStarted {
        checkpoint: CheckpointId(1),
        restore: CheckpointId(0),
        history: HistoryAnchor::Fresh,
    });
    let failed = after(EventKind::UndoFailed {
        checkpoint: CheckpointId(1),
        reason: text("dirty tree"),
    });
    let undone = after(EventKind::TurnsUndone {
        checkpoint: CheckpointId(1),
        restored: CheckpointId(0),
    });
    let (_, focus) = run(
        undone.clone(),
        Vec::from([Intent::Session(SessionIntent::Mounted(Mount::Undone))]),
    );
    assert_eq!(
        (
            sent(&effects),
            pane(&undoing).map(|shown| (shown.undo, shown.status)),
            pane(&failed).map(|shown| shown.undo),
            pane(&undone).map(|shown| (shown.undo, shown.entries.len())),
            focus,
        ),
        (
            Vec::from([CommandBody::UndoTurn {
                session: SESSION,
                checkpoint: CheckpointId(1)
            }]),
            Some((
                UndoNote::Undoing(
                    "Undoing this turn and all later file and conversation changes…"
                ),
                text("undoing")
            )),
            Some(UndoNote::Failed {
                text: text(
                    "Undo needs recovery: dirty tree. Other actions are blocked until recovery finishes."
                ),
                checkpoint: CheckpointId(1),
            }),
            Some((
                UndoNote::Undone {
                    key: text("undo-done-7"),
                    text: "Turn and all later file and conversation changes undone.",
                },
                0
            )),
            Vec::from([Effect::Focus(Focus::AfterUndo)]),
        )
    );
}

#[test]
fn a_refusal_is_the_notice_until_dismissed() {
    let (interrupted, effects) =
        opened(Vec::from([Intent::Session(SessionIntent::Interrupt)]));
    let (state, _) = run(
        interrupted,
        Vec::from([answer(
            &effects,
            Err(text("Not connected to the host yet.")),
        )]),
    );
    let (dismissed, _) = run(state.clone(), Vec::from([Intent::DismissNotice]));
    assert_eq!(
        (
            sent(&effects),
            pane(&state).and_then(|shown| shown.notice),
            pane(&dismissed).and_then(|shown| shown.notice),
        ),
        (
            Vec::from([CommandBody::Interrupt { session: SESSION }]),
            Some(text("Not connected to the host yet.")),
            None,
        )
    );
}

fn sign_in_of(state: &State) -> Option<crate::SignInView> {
    pane_of(state)
        .and_then(|pane| pane.instances.into_iter().next())
        .map(|section| section.sign_in)
}

fn sign_in(intent: CredentialIntent) -> Intent {
    Intent::Credential(HOST, intent)
}

/// The credentials screen of a host whose Claude instance signs in at
/// `step`, if a sign-in runs on it.
fn signing(progress: Option<SignInStep>) -> State {
    let mut intents = Vec::from([
        connection(HOST, LinkState::Live, None),
        roster(claude(signed_in())),
    ]);
    intents.extend(progress.map(|step| {
        Intent::Link(
            HOST,
            Update::Feed(Feed::SignIn(SignIn {
                instance: id("claude"),
                step,
            })),
        )
    }));
    intents.push(Intent::Show(Place::Credentials));
    run(paired(), intents).0
}

#[test]
fn a_sign_in_starts_from_the_instance_and_takes_the_pasted_code_once() {
    let idle = signing(None);
    let (_, started) = run(
        idle.clone(),
        Vec::from([sign_in(CredentialIntent::StartSignIn(id("claude")))]),
    );
    let open = signing(Some(SignInStep::Open {
        url: text("https://sign.example/in?state=1"),
        code: None,
        paste: true,
    }));
    let (typed, _) = run(
        open,
        Vec::from([sign_in(CredentialIntent::TypeSignInCode(
            id("claude"),
            text(" abc#def "),
        ))]),
    );
    let (submitted, sent_code) = run(
        typed.clone(),
        Vec::from([sign_in(CredentialIntent::SubmitSignInCode(id("claude")))]),
    );
    let (accepted, _) = run(
        submitted.clone(),
        Vec::from([answer(&sent_code, Ok(CommandResult::Accepted))]),
    );
    let paste = |state: &State| {
        sign_in_of(state)
            .and_then(|view| view.paste)
            .map(|form| (form.value, form.submit_disabled))
    };
    assert_eq!(
        (
            sign_in_of(&idle).and_then(|view| view.start),
            sent(&started),
            sign_in_of(&typed).map(|view| (view.link, view.progress)),
            paste(&typed),
            sent(&sent_code),
            paste(&submitted),
            paste(&accepted),
            credentials(&accepted).first().map(|shown| shown.1.clone()),
        ),
        (
            Some(crate::SignInButton {
                text: text("Sign in"),
                label: text("Sign in: Claude Code"),
                disabled: false,
            }),
            Vec::from([CommandBody::StartSignIn {
                instance: id("claude")
            }]),
            Some((
                Some(text("https://sign.example/in?state=1")),
                Some(Hint {
                    class: "hint muted",
                    role: Some("status"),
                    text: text(
                        "Open the sign-in page and sign in. Then paste the code it shows here."
                    ),
                })
            )),
            Some((text(" abc#def "), false)),
            Vec::from([CommandBody::SubmitSignInCode {
                instance: id("claude"),
                code: SignInCode(text("abc#def")),
            }]),
            Some((String::new(), true)),
            Some((String::new(), true)),
            Some(None),
        )
    );
}

#[test]
fn a_device_code_is_shown_to_copy_and_each_step_says_where_it_stands() {
    let device = signing(Some(SignInStep::Open {
        url: text("https://sign.example/device"),
        code: Some(text("ABCD-12345")),
        paste: false,
    }));
    let (_, cancelled) = run(
        device.clone(),
        Vec::from([sign_in(CredentialIntent::CancelSignIn(id("claude")))]),
    );
    let code = |state: &State| {
        sign_in_of(state)
            .and_then(|view| view.code)
            .map(|shown| shown.code)
    };
    let progress = |step| {
        sign_in_of(&signing(Some(step))).map(|view| {
            (
                view.start.map(|button| button.text),
                view.progress.map(|hint| (hint.class, hint.text)),
                view.cancel.is_some(),
            )
        })
    };
    assert_eq!(
        (
            code(&device),
            sign_in_of(&device).map(|view| view.paste.is_none()),
            sent(&cancelled),
        ),
        (
            Some(text("ABCD-12345")),
            Some(true),
            Vec::from([CommandBody::CancelSignIn {
                instance: id("claude")
            }]),
        )
    );
    assert_eq!(
        [
            progress(SignInStep::Starting),
            progress(SignInStep::Finishing),
            progress(SignInStep::SignedIn {
                instances: claude(signed_in())
            }),
            progress(SignInStep::Failed {
                reason: text("the code expired")
            }),
        ],
        [
            Some((
                None,
                Some(("hint muted", text("Starting the sign-in on desk…"))),
                true
            )),
            Some((
                None,
                Some(("hint muted", text("Finishing the sign-in on desk…"))),
                true
            )),
            Some((
                Some(text("Sign in again")),
                Some(("hint muted", text("Signed in on desk."))),
                false
            )),
            Some((
                Some(text("Sign in")),
                Some((
                    "hint error",
                    text("The sign-in failed: the code expired")
                )),
                false
            )),
        ]
    );
}

#[test]
fn a_session_on_an_issue_finishes_and_says_what_landed() {
    let (plain, _) = opened(Vec::new());
    assert_eq!(pane(&plain).map(|shown| shown.finish), Some(false));
    let (on_issue, _) = opened(Vec::from([event(
        3,
        EventKind::OnIssue {
            issue: proto::IssueId(5),
        },
    )]));
    assert_eq!(pane(&on_issue).map(|shown| shown.finish), Some(true));
    let (finished, effects) = run(
        on_issue,
        Vec::from([
            Intent::Session(SessionIntent::Finish),
            event(
                4,
                EventKind::SessionFinished {
                    finished: proto::Finished::Landed {
                        commits: 1,
                        kept: proto::Kept::Pushed {
                            remote: text("git@example.com:me/alpha.git"),
                        },
                    },
                },
            ),
        ]),
    );
    assert_eq!(
        sent(&effects),
        [CommandBody::FinishSession { session: SESSION }]
    );
    assert_eq!(
        rows(&finished).last(),
        Some(&Row::Finished {
            outcome: "landed",
            text: text(
                "Finished: landed 1 commit on the issue's branch; pushed to git@example.com:me/alpha.git."
            ),
        })
    );
}
