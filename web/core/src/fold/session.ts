import {
  Approval,
  Change,
  Item,
  RequestState,
  ToolState,
  changeItems,
  hasCall,
  openApproval,
  settle,
  streamedItems,
} from "#core/fold/transcript";
import type {
  ChecklistEntry,
  ClientName,
  CloseReason,
  Effort,
  EventKind,
  HostSeq,
  InstanceId,
  IssueKey,
  ModelId,
  Output,
  RequestId,
  Resolution,
  SessionDirectory,
  ToolCallId,
  ToolKind,
  TurnOutcome,
} from "#core/schemas/index";
import { Data, Match } from "effect";
import type { ReplyState, Streamed } from "#core/fold/transcript";
import { applySubagentEvent, changeSubagents, stopSubagents } from "#core/fold/subagent";
import type { Subagent } from "#core/fold/subagent";
import type { Unit } from "#core/data";

type SessionStatus = Data.TaggedEnum<{
  readonly Starting: Unit;
  readonly Idle: Unit;
  readonly Running: Unit;
  readonly Closed: { readonly reason: CloseReason };
}>;

const SessionStatus = Data.taggedEnum<SessionStatus>();

type FinishState = "Ready" | "Finishing" | "Retrying" | "Done";

interface Session {
  readonly instance: InstanceId;
  readonly directory: SessionDirectory;
  readonly model: ModelId | undefined;
  readonly effort: Effort | undefined;
  readonly status: SessionStatus;
  readonly issue: IssueKey | undefined;
  readonly finish: FinishState;
  readonly checklist: ReadonlyArray<ChecklistEntry>;
  readonly items: ReadonlyArray<Item>;
  readonly subagents: ReadonlyArray<Subagent>;
  readonly activity: HostSeq;
}

type Created = Extract<EventKind, { readonly SessionCreated: unknown }>["SessionCreated"];

type Step = (session: Session) => Session;

const createdSession = (created: Created, seq: HostSeq): Session => ({
  activity: seq,
  checklist: [],
  directory: created.directory,
  effort: created.effort ?? undefined,
  finish: "Ready",
  instance: created.instance,
  issue: undefined,
  items: [],
  model: created.model ?? undefined,
  status: SessionStatus.Starting(),
  subagents: [],
});

const pushed = (session: Session, item: Item): Session => ({
  ...session,
  items: [...settle(session.items), item],
});

const amended =
  (next: Change): Step =>
  (session) => ({ ...session, items: changeItems(session.items, next) });

const streamed =
  (tag: Streamed, text: string, state: ReplyState): Step =>
  (session) => ({
    ...session,
    items: streamedItems(session.items, { state, tag, text }),
  });

const ready: Step = (session) =>
  SessionStatus.$is("Closed")(session.status)
    ? pushed(
        { ...session, status: SessionStatus.Idle() },
        Item.Continued({ reason: session.status.reason }),
      )
    : { ...session, status: SessionStatus.Idle() };

const userMessage =
  (text: string, by: ClientName): Step =>
  (session) =>
    pushed(
      {
        ...session,
        finish: session.finish === "Done" ? "Ready" : session.finish,
        status: SessionStatus.Running(),
      },
      Item.User({ by, text }),
    );

const turnCompleted =
  (outcome: TurnOutcome): Step =>
  (session) =>
    pushed(
      { ...amended(Change.End())(session), status: SessionStatus.Idle() },
      Item.TurnCompleted({ outcome }),
    );

const closed =
  (reason: CloseReason): Step =>
  (session) => ({
    ...session,
    items: settle(changeItems(session.items, Change.End())),
    status: SessionStatus.Closed({ reason }),
    subagents: stopSubagents(session.subagents),
  });

const resolved =
  (request: RequestId, resolution: Resolution): Step =>
  (session) => {
    const next = Change.Resolve({ request, resolution });
    return {
      ...session,
      items: changeItems(session.items, next),
      subagents: changeSubagents(session.subagents, next),
    };
  };

const toolStarted =
  (call: ToolCallId, tool: ToolKind): Step =>
  (session) =>
    pushed(
      session,
      Item.Tool({
        approval: Approval.NotAsked(),
        call,
        state: ToolState.Running(),
        tool,
      }),
    );

const requestOpened =
  (body: Extract<EventKind, { readonly RequestOpened: unknown }>["RequestOpened"]): Step =>
  (session) => {
    const { call, tool } = body;
    const approval = openApproval(body.request, body.reason, body.for_session);
    return hasCall(session.items, call)
      ? amended(Change.Ask({ approval, call }))(session)
      : pushed(session, Item.Tool({ approval, call, state: ToolState.Running(), tool }));
  };

const unchanged: Step = (session) => session;

const errorItem = Item.Error;

const errored =
  (message: string): Step =>
  (session) =>
    pushed(session, errorItem({ message }));

const output = Match.type<Output>().pipe(
  Match.withReturnType<Step>(),
  Match.when({ AssistantDelta: Match.any }, ({ AssistantDelta: { text } }) =>
    streamed("Reply", text, "Streaming"),
  ),
  Match.when({ AssistantMessage: Match.any }, ({ AssistantMessage: { text } }) =>
    streamed("Reply", text, "Done"),
  ),
  Match.when({ ReasoningDelta: Match.any }, ({ ReasoningDelta: { text } }) =>
    streamed("Reasoning", text, "Streaming"),
  ),
  Match.when({ Reasoning: Match.any }, ({ Reasoning: { text } }) =>
    streamed("Reasoning", text, "Done"),
  ),
  Match.when({ ToolCall: Match.any }, ({ ToolCall: { call, phase } }) =>
    "Finished" in phase
      ? amended(Change.Finish({ call, outcome: phase.Finished.outcome }))
      : toolStarted(call, phase.Started.tool),
  ),
  Match.when({ Error: Match.any }, ({ Error: { message } }) => errored(message)),
  Match.exhaustive,
);

const step = Match.type<EventKind>()
  .pipe(
    Match.withReturnType<Step>(),
    Match.when({ UserMessage: Match.any }, ({ UserMessage: { text, by } }) =>
      userMessage(text, by),
    ),
    Match.when({ Agent: Match.any }, ({ Agent }) => output(Agent)),
    Match.when({ RequestOpened: Match.any }, ({ RequestOpened: body }) => requestOpened(body)),
    Match.when(
      { QuestionAsked: Match.any },
      ({ QuestionAsked: { request, questions } }) =>
        (session: Session): Session =>
          pushed(session, Item.Question({ questions, request, state: RequestState.Open() })),
    ),
    Match.when({ RequestResolved: Match.any }, ({ RequestResolved: { request, resolution } }) =>
      resolved(request, resolution),
    ),
    Match.when({ TurnCompleted: Match.any }, ({ TurnCompleted: { outcome } }) =>
      turnCompleted(outcome),
    ),
    Match.when({ Error: Match.any }, ({ Error: { message } }) => errored(message)),
    Match.when(
      { Subagent: Match.any },
      ({ Subagent: { agent, event } }) =>
        (session: Session): Session => ({
          ...session,
          subagents: applySubagentEvent(session.subagents, agent, event),
        }),
    ),
  )
  .pipe(
    Match.when("SessionReady", () => ready),
    Match.when({ NativeTurnStarted: Match.any }, () => (session: Session): Session => ({
      ...session,
      status: SessionStatus.Running(),
    })),
    Match.when({ SessionClosed: Match.any }, ({ SessionClosed: { reason } }) => closed(reason)),
    Match.when(
      { ModelChanged: Match.any },
      ({ ModelChanged: { model, effort, reason } }) =>
        (session: Session): Session =>
          pushed(
            { ...session, effort: effort ?? undefined, model: model ?? undefined },
            Item.ModelChanged({ reason }),
          ),
    ),
    Match.when(
      { ChecklistChanged: Match.any },
      ({ ChecklistChanged: { checklist } }) =>
        (session: Session): Session => ({ ...session, checklist }),
    ),
    Match.when(
      { EffortChanged: Match.any },
      ({ EffortChanged: { effort } }) =>
        (session: Session): Session => ({ ...session, effort: effort ?? undefined }),
    ),
    Match.when({ OnIssue: Match.any }, ({ OnIssue: { issue } }) => (session: Session): Session => ({
      ...session,
      issue,
    })),
    Match.when(
      "FinishStarted",
      () =>
        (session: Session): Session =>
          pushed({ ...session, finish: "Finishing" }, Item.Finishing()),
    ),
    Match.when("FinishInterrupted", () => (session: Session): Session => ({
      ...session,
      finish: session.finish === "Finishing" ? "Retrying" : session.finish,
    })),
    Match.when(
      { SessionFinished: Match.any },
      ({ SessionFinished: { finished } }) =>
        (session: Session): Session =>
          pushed({ ...session, finish: "Done" }, Item.Finished({ finished })),
    ),
    Match.when({ WorkspaceCreated: Match.any }, () => unchanged),
    Match.when({ SnapshotRecorded: Match.any }, () => unchanged),
    Match.when({ SessionCreated: Match.any }, () => unchanged),
    Match.when({ SessionRemoved: Match.any }, () => unchanged),
    Match.exhaustive,
  );

const applyKind = (session: Session, kind: EventKind): Session => step(kind)(session);

export { applyKind, createdSession, type Session, SessionStatus };
