// A session folded from its events.
import { Data, Match } from "effect";
import type {
	ChecklistEntry,
	ClientName,
	CloseReason,
	Effort,
	EventKind,
	HostPath,
	HostSeq,
	InstanceId,
	IssueId,
	ModelId,
	Output,
	RequestId,
	Resolution,
	ToolCallId,
	ToolKind,
	TurnOutcome,
} from "../proto/index.ts";
import {
	applySubagentEvent,
	changeSubagents,
	type Subagent,
	stopSubagents,
} from "./subagent.ts";
import {
	Approval,
	Change,
	changeItems,
	hasCall,
	Item,
	openApproval,
	type ReplyState,
	RequestState,
	type Streamed,
	settle,
	streamedItems,
	ToolState,
	type Unit,
} from "./transcript.ts";

type SessionStatus = Data.TaggedEnum<{
	readonly Starting: Unit;
	readonly Idle: Unit;
	readonly Running: Unit;
	readonly Closed: { readonly reason: CloseReason };
}>;

const SessionStatus = Data.taggedEnum<SessionStatus>();

// Where a session on an issue stands with finishing: a message after a finish makes it ready
// again, while its own summary turn doesn't.
type FinishState = "Ready" | "Finishing" | "Retrying" | "Done";

interface Session {
	readonly instance: InstanceId;
	readonly directory: HostPath;
	// What the agent runs with; null is the CLI's own default.
	readonly model: ModelId | null;
	readonly effort: Effort | null;
	readonly status: SessionStatus;
	readonly issue: IssueId | null;
	readonly finish: FinishState;
	readonly checklist: readonly ChecklistEntry[];
	readonly items: readonly Item[];
	// The sub-agents its agent started, in the order they started.
	readonly subagents: readonly Subagent[];
	// The last stored event that touched it.
	readonly activity: HostSeq;
}

type Created = Extract<
	EventKind,
	{ readonly SessionCreated: unknown }
>["SessionCreated"];

type Step = (session: Session) => Session;

const createdSession = (created: Created, seq: HostSeq): Session => ({
	instance: created.instance,
	directory: created.directory,
	model: created.model,
	effort: created.effort,
	status: SessionStatus.Starting(),
	issue: null,
	finish: "Ready",
	checklist: [],
	items: [],
	subagents: [],
	activity: seq,
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
		items: streamedItems(session.items, tag, text, state),
	});

// A session that closed and is ready again was continued.
const ready: Step = (session) =>
	session.status._tag === "Closed"
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
				status: SessionStatus.Running(),
				finish: session.finish === "Done" ? "Ready" : session.finish,
			},
			Item.User({ text, by }),
		);

const turnCompleted =
	(outcome: TurnOutcome): Step =>
	(session) =>
		pushed(
			{ ...amended(Change.End())(session), status: SessionStatus.Idle() },
			Item.TurnCompleted({ outcome }),
		);

// Nothing still running will finish, and nothing still open will be answered.
const closed =
	(reason: CloseReason): Step =>
	(session) => ({
		...session,
		items: settle(changeItems(session.items, Change.End())),
		subagents: stopSubagents(session.subagents),
		status: SessionStatus.Closed({ reason }),
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
				call,
				tool,
				state: ToolState.Running(),
				approval: Approval.NotAsked(),
			}),
		);

// A request joins its tool call's card; a request for a call never seen started gets a card of
// its own.
const requestOpened =
	(
		body: Extract<
			EventKind,
			{ readonly RequestOpened: unknown }
		>["RequestOpened"],
	): Step =>
	(session) => {
		const { call, tool } = body;
		const approval = openApproval(body.request, body.reason, body.for_session);
		return hasCall(session.items, call)
			? amended(Change.Ask({ call, approval }))(session)
			: pushed(
					session,
					Item.Tool({ call, tool, state: ToolState.Running(), approval }),
				);
	};

const unchanged: Step = (session) => session;

const errored =
	(message: string): Step =>
	(session) =>
		pushed(session, Item.Error({ message }));

const output = Match.type<Output>().pipe(
	Match.withReturnType<Step>(),
	Match.when({ AssistantDelta: Match.any }, ({ AssistantDelta: { text } }) =>
		streamed("Reply", text, "Streaming"),
	),
	Match.when(
		{ AssistantMessage: Match.any },
		({ AssistantMessage: { text } }) => streamed("Reply", text, "Done"),
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
	Match.when({ Error: Match.any }, ({ Error: { message } }) =>
		errored(message),
	),
	Match.exhaustive,
);

const step = Match.type<EventKind>()
	.pipe(
		Match.withReturnType<Step>(),
		Match.when({ UserMessage: Match.any }, ({ UserMessage: { text, by } }) =>
			userMessage(text, by),
		),
		Match.when({ Agent: Match.any }, ({ Agent }) => output(Agent)),
		Match.when({ RequestOpened: Match.any }, ({ RequestOpened: body }) =>
			requestOpened(body),
		),
		Match.when(
			{ QuestionAsked: Match.any },
			({ QuestionAsked: { request, questions } }) =>
				(session: Session) =>
					pushed(
						session,
						Item.Question({ request, questions, state: RequestState.Open() }),
					),
		),
		Match.when(
			{ RequestResolved: Match.any },
			({ RequestResolved: { request, resolution } }) =>
				resolved(request, resolution),
		),
		Match.when({ TurnCompleted: Match.any }, ({ TurnCompleted: { outcome } }) =>
			turnCompleted(outcome),
		),
		Match.when({ Error: Match.any }, ({ Error: { message } }) =>
			errored(message),
		),
		Match.when(
			{ Subagent: Match.any },
			({ Subagent: { agent, event } }) =>
				(session: Session) => ({
					...session,
					subagents: applySubagentEvent(session.subagents, agent, event),
				}),
		),
	)
	.pipe(
		Match.when("SessionReady", () => ready),
		Match.when({ SessionClosed: Match.any }, ({ SessionClosed: { reason } }) =>
			closed(reason),
		),
		Match.when(
			{ ModelChanged: Match.any },
			({ ModelChanged: { model, effort, reason } }) =>
				(session: Session) =>
					pushed({ ...session, model, effort }, Item.ModelChanged({ reason })),
		),
		Match.when(
			{ ChecklistChanged: Match.any },
			({ ChecklistChanged: { checklist } }) =>
				(session: Session) => ({ ...session, checklist }),
		),
		Match.when(
			{ EffortChanged: Match.any },
			({ EffortChanged: { effort } }) =>
				(session: Session) => ({ ...session, effort }),
		),
		Match.when(
			{ OnIssue: Match.any },
			({ OnIssue: { issue } }) =>
				(session: Session) => ({ ...session, issue }),
		),
		Match.when(
			"FinishStarted",
			() => (session: Session) =>
				pushed({ ...session, finish: "Finishing" }, Item.Finishing()),
		),
		Match.when("FinishInterrupted", () => (session: Session) => ({
			...session,
			finish: session.finish === "Finishing" ? "Retrying" : session.finish,
		})),
		Match.when(
			{ SessionFinished: Match.any },
			({ SessionFinished: { finished } }) =>
				(session: Session) =>
					pushed({ ...session, finish: "Done" }, Item.Finished({ finished })),
		),
		// The thin app shows no workspace, snapshots or native turns; the host creates and removes
		// sessions.
		Match.when({ WorkspaceCreated: Match.any }, () => unchanged),
		Match.when({ SnapshotRecorded: Match.any }, () => unchanged),
		Match.when({ NativeTurnStarted: Match.any }, () => unchanged),
		Match.when({ SessionCreated: Match.any }, () => unchanged),
		Match.when({ SessionRemoved: Match.any }, () => unchanged),
		Match.exhaustive,
	);

const applyKind = (session: Session, kind: EventKind): Session =>
	step(kind)(session);

export { applyKind, createdSession, type Session, SessionStatus };
