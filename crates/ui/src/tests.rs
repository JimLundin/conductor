use alloc::collections::BTreeMap;

use client::link::{LinkState, Status, Update};
use proto::{
    Auth, AuthToken, Availability, Catalog, Checkpoint, CheckpointDiff,
    CheckpointFile, CheckpointId, ClientName, CloseReason, CommandBody,
    CommandResult, Credential, CredentialForm, Decision, DiffExtent, Effort,
    Event, EventKind, Field, FieldKind, FilePatch, HistoryAnchor, HostPath,
    HostSeq, ModelChoice, ModelId, Models, Provider, ProviderInfo, Providers,
    Question, QuestionOption, Rejection, RequestId, Resolution, Selection,
    SessionId, Ticket, TicketError, ToolCallId, ToolKind, ToolOutcome,
    ToolPhase, TurnOutcome, UndoAvailability,
};

use crate::credentials::Done;
use crate::{
    Action, Ask, CredentialIntent, Detail, Diff, DiffFile, DiffPanel, Effect,
    EffortControl, Focus, Footer, FormIntent, Hint, HostScreen, Intent,
    Keystroke, Listing, Mount, NewSessionForm, Opening, Patch, Place,
    QuestionBody, Reply, ReplyTo, Row, Screen, SessionIntent, SessionPane,
    State, ToolDetail, ToolLine, TurnChanges, UndoNote, sends, update, view,
};

const SESSION: SessionId = SessionId(7);

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

/// Paired with a host whose catalog arrived but which isn't live.
fn paired() -> State {
    let (state, _) = update(
        State::new(Ok(Opening::Host(ticket()))),
        Intent::Link(Update::Status(Status {
            catalog: catalog(),
            ..Status::default()
        })),
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
    Intent::Link(Update::Event(Event {
        seq: HostSeq(seq),
        session: SESSION,
        kind,
    }))
}

fn created() -> EventKind {
    EventKind::SessionCreated {
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

fn status(providers: Providers) -> Intent {
    Intent::Link(Update::Status(Status {
        state: LinkState::Live,
        host: Some(text("desk")),
        providers: Some(providers),
        catalog: catalog(),
        error: None,
    }))
}

fn claude(availability: Availability) -> Providers {
    Providers(BTreeMap::from([(Provider::Claude, availability)]))
}

/// A session that is ready, shown in the detail pane.
fn opened(more: Vec<Intent>) -> (State, Vec<Effect>) {
    let shown = Vec::from([
        event(1, created()),
        event(2, EventKind::SessionReady),
        Intent::Show(Place::Session(SESSION)),
    ]);
    run(paired(), shown.into_iter().chain(more).collect())
}

fn screen(state: &State) -> Option<HostScreen> {
    match view(state) {
        Screen::Host(host) => Some(*host),
        Screen::Pair(_) => None,
    }
}

fn pane(state: &State) -> Option<SessionPane> {
    screen(state).and_then(|host| match host.detail {
        Detail::Session(pane) => Some(*pane),
        Detail::Placeholder(_)
        | Detail::NewSession(_)
        | Detail::Credentials(_) => None,
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
        | Detail::Credentials(_)
        | Detail::Session(_) => None,
    })
}

fn sent(effects: &[Effect]) -> Vec<CommandBody> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Send { body, .. } => Some(body.clone()),
            Effect::SaveTicket(_)
            | Effect::ForgetTicket
            | Effect::Focus(_)
            | Effect::ShowEffort(_) => None,
        })
        .collect()
}

const fn reply(to: ReplyTo) -> Reply {
    Reply { visit: 1, to }
}

fn line(state: &'static str, command: &str) -> ToolLine {
    ToolLine {
        state,
        detail: ToolDetail::Command(text(command)),
    }
}

#[test]
fn pairing_saves_a_valid_link_reports_a_bad_one_and_forgets_the_host() {
    let refused = State::new(Err(TicketError::Truncated));
    let (typed, _) = run(refused, Vec::from([Intent::TypeJoin(text(" "))]));
    let (joined, saved) =
        run(typed.clone(), Vec::from([Intent::Join(Ok(ticket()))]));
    let (forgot, forgotten) =
        run(joined.clone(), Vec::from([Intent::ChangeHost]));
    let pair = |state: &State| match view(state) {
        Screen::Pair(pair) => Some((pair.connect_disabled, pair.error)),
        Screen::Host(_) => None,
    };
    let invalid =
        Some(text("That join link is not valid: ticket is truncated"));
    assert_eq!(
        (
            pair(&typed),
            screen(&joined).map(|host| host.ticket),
            saved,
            pair(&forgot),
            forgotten,
        ),
        (
            Some((true, invalid.clone())),
            Some(ticket()),
            Vec::from([Effect::SaveTicket(ticket())]),
            Some((true, invalid)),
            Vec::from([Effect::ForgetTicket]),
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
        Vec::from([Intent::Link(Update::Reset(proto::Epoch(2)))]),
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
    let row = match screen(&state).map(|host| host.list.listing) {
        Some(Listing::Rows(rows)) => rows.into_iter().next().map(|row| {
            (row.status, row.current, row.title, row.sub, row.badge)
        }),
        Some(Listing::Empty(_)) | None => None,
    };
    assert_eq!(
        (row, screen(&paired()).map(|host| host.list.listing)),
        (
            Some((
                text("approval"),
                true,
                text("conductor"),
                text("Claude Code, needs you"),
                text("1")
            )),
            Some(Listing::Empty(
                "No sessions yet. Start one in a directory on the host."
            )),
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
        (shown(&open), shown(&resolved), decided),
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
            Vec::from([
                Effect::Send {
                    body: CommandBody::Resolve {
                        session: SESSION,
                        request: RequestId(3),
                        decision: Decision::ApproveForSession,
                    },
                    reply: reply(ReplyTo::Notice),
                },
                Effect::Focus(Focus::Element(text("request-3"))),
            ]),
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
            effects,
            pane(&continued).map(|shown| shown.status),
        ),
        (
            Some(Footer::Closed {
                text: text("Stopped: reset"),
                resume: true
            }),
            Vec::from([
                Effect::Send {
                    body: CommandBody::Continue { session: SESSION },
                    reply: reply(ReplyTo::Notice),
                },
                Effect::Focus(Focus::Composer),
            ]),
            Some(text("idle")),
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
        status(claude(signed_in())),
        Intent::Session(SessionIntent::PickEffort(text("high"))),
        Intent::Session(SessionIntent::PickEffort(String::new())),
    ]));
    let (refused, put_back) = run(
        state.clone(),
        Vec::from([Intent::Replied(
            reply(ReplyTo::Effort(None)),
            Ok(CommandResult::Rejected(Rejection::TurnInProgress)),
        )]),
    );
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
            effects,
            put_back,
            pane(&refused).and_then(|shown| shown.notice),
        ),
        (
            Some(Vec::from([
                (String::new(), text("default effort"), true, true),
                (text("low"), text("low effort"), false, false),
                (text("high"), text("high effort"), false, false),
            ])),
            Vec::from([Effect::Send {
                body: CommandBody::SetEffort {
                    session: SESSION,
                    effort: Effort::High
                },
                reply: reply(ReplyTo::Effort(None)),
            }]),
            Vec::from([Effect::ShowEffort(String::new())]),
            Some(text("a turn is already running")),
        )
    );
}

#[test]
fn the_new_session_form_validates_and_shows_refusals_where_they_belong() {
    let providers = Providers(BTreeMap::from([
        (Provider::Claude, signed_in()),
        (
            Provider::Codex,
            Availability::Unavailable {
                reason: text("no codex"),
            },
        ),
    ]));
    let (empty, _) = run(
        paired(),
        Vec::from([status(providers), Intent::Show(Place::NewSession)]),
    );
    let (codex, _) = run(
        empty.clone(),
        Vec::from([Intent::Form(FormIntent::PickProvider(Provider::Codex))]),
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
    let answered = |answer| {
        run(
            starting.clone(),
            Vec::from([Intent::Replied(reply(ReplyTo::Start), Ok(answer))]),
        )
        .0
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
            form(&codex).and_then(|shown| shown.provider_hint),
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
                text: text("Codex can't run on this host: no codex"),
            }),
            Some((true, "Starting…")),
            Vec::from([CommandBody::StartSession {
                provider: Provider::Claude,
                directory: HostPath(text("/src/x")),
                model: Some(ModelId(text("opus"))),
                effort: Some(Effort::High),
            }]),
            Some((
                true,
                Some(Hint {
                    class: "hint error",
                    role: Some("alert"),
                    text: text(
                        "There is no directory at that path on the host."
                    ),
                })
            )),
            Some((false, "Start session")),
            Some("detail"),
        )
    );
}

type Shown = (String, Option<bool>, bool, Option<String>, Vec<String>);

fn credentials(state: &State) -> Vec<Shown> {
    screen(state).map_or_else(Vec::new, |host| match host.detail {
        Detail::Credentials(pane) => pane
            .providers
            .into_iter()
            .map(|provider| {
                (
                    provider.state,
                    provider.clear.map(|clear| clear.disabled),
                    provider.forms.iter().any(|form| form.save_disabled),
                    provider.outcome.map(|hint| hint.text),
                    provider
                        .forms
                        .into_iter()
                        .flat_map(|form| form.fields)
                        .map(|field| field.value)
                        .collect(),
                )
            })
            .collect(),
        Detail::Placeholder(_) | Detail::NewSession(_) | Detail::Session(_) => {
            Vec::new()
        }
    })
}

#[test]
fn a_secret_is_sent_once_and_only_a_gateway_can_be_cleared() {
    let gateway = Availability::Available {
        auth: Auth::Stored {
            kind: text("gateway"),
            url: Some(text("https://gw.example")),
        },
        models: Models::none(),
    };
    let fill = |field: &str, value: &str| {
        Intent::Credential(CredentialIntent::Type {
            provider: Provider::Claude,
            kind: text("gateway"),
            field: text(field),
            text: text(value),
        })
    };
    let (filled, _) = run(
        paired(),
        Vec::from([
            status(claude(gateway)),
            Intent::Show(Place::Credentials),
            fill("base_url", " https://gw.example "),
            fill("token", "secret"),
        ]),
    );
    let (saving, saved) = run(
        filled.clone(),
        Vec::from([Intent::Credential(CredentialIntent::Save {
            provider: Provider::Claude,
            kind: text("gateway"),
        })]),
    );
    let (stored, _) = run(
        saving.clone(),
        Vec::from([Intent::Replied(
            reply(ReplyTo::Credential(Provider::Claude, Done::Stored)),
            Ok(CommandResult::Providers(Providers(BTreeMap::new()))),
        )]),
    );
    let (_, cleared) = run(
        stored.clone(),
        Vec::from([Intent::Credential(CredentialIntent::Clear(
            Provider::Claude,
        ))]),
    );
    let blank = core::iter::repeat_n(String::new(), 4).collect::<Vec<String>>();
    assert_eq!(
        (
            credentials(&filled).first().map(|form| form.2),
            credentials(&saving),
            sent(&saved),
            credentials(&stored).first().map(|form| form.3.clone()),
            cleared.last().cloned(),
        ),
        (
            Some(false),
            Vec::from([
                (
                    text("Gateway at https://gw.example"),
                    Some(true),
                    true,
                    Some(text("Sending…")),
                    Vec::from([text(" https://gw.example "), String::new()]),
                ),
                (text("Waiting for the host"), None, true, None, blank),
            ]),
            Vec::from([CommandBody::SetCredential {
                credential: Credential {
                    provider: Provider::Claude,
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
            Some(Effect::Focus(Focus::Element(text("claude-credential")))),
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

fn diff_reply() -> Reply {
    reply(ReplyTo::Diff {
        checkpoint: CheckpointId(1),
        request: 1,
    })
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
    let loaded_with = |answer| {
        run(
            loading.clone(),
            Vec::from([Intent::Replied(diff_reply(), answer)]),
        )
        .0
    };
    let loaded = loaded_with(Ok(CommandResult::Diff(diff())));
    let (busy, _) = run(loaded.clone(), Vec::from([event(6, said("again"))]));
    let (not_offered, _) = turn(UndoAvailability::Unavailable {
        reason: text("no worktree"),
    });
    let unavailable = run(
        not_offered,
        Vec::from([Intent::Replied(
            diff_reply(),
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
fn a_refusal_is_the_notice_until_dismissed_and_a_left_pane_drops_late_answers()
{
    let (state, effects) = opened(Vec::from([
        Intent::Session(SessionIntent::Interrupt),
        Intent::Replied(
            reply(ReplyTo::Notice),
            Err(text("Not connected to the host yet.")),
        ),
    ]));
    let (dismissed, _) = run(state.clone(), Vec::from([Intent::DismissNotice]));
    let back = Vec::from([
        Intent::Show(Place::Sessions),
        Intent::Show(Place::Session(SESSION)),
    ]);
    let (returned, _) = run(state.clone(), back.clone());
    let (late, _) = run(
        state.clone(),
        core::iter::once(Intent::Session(SessionIntent::ShowChanges(
            CheckpointId(1),
        )))
        .chain(back)
        .chain([Intent::Replied(
            diff_reply(),
            Ok(CommandResult::Diff(diff())),
        )])
        .collect(),
    );
    assert_eq!(
        (
            sent(&effects),
            pane(&state).and_then(|shown| shown.notice),
            pane(&dismissed).and_then(|shown| shown.notice),
            pane(&late) == pane(&returned),
        ),
        (
            Vec::from([CommandBody::Interrupt { session: SESSION }]),
            Some(text("Not connected to the host yet.")),
            None,
            true,
        )
    );
}
