// A session's sub-agents: children with no session of their own, whose events are its.
import { Data, Match } from "effect";
import type {
	ChecklistEntry,
	SubagentActivity,
	SubagentEvent,
	SubagentId,
	SubagentOutcome,
	ToolCallId,
	ToolKind,
} from "../schemas/index.ts";
import {
	Approval,
	Change,
	changeItems,
	hasCall,
	Item,
	openApproval,
	RequestState,
	ToolState,
	type Unit,
} from "./transcript.ts";

type SubagentStatus = Data.TaggedEnum<{
	readonly Running: Unit;
	readonly Ended: { readonly outcome: SubagentOutcome };
	// The session closed while it ran.
	readonly Stopped: Unit;
}>;

const SubagentStatus = Data.taggedEnum<SubagentStatus>();

// A sub-agent, shown under the tool call that started it.
interface Subagent {
	readonly id: SubagentId;
	readonly call: ToolCallId;
	readonly description: string;
	readonly kind: string | null;
	readonly checklist: readonly ChecklistEntry[];
	readonly status: SubagentStatus;
	// Its own transcript, whose last reply is its report.
	readonly items: readonly Item[];
}

type Subagents = readonly Subagent[];

type Update = (subagent: Subagent) => Subagent;

const pushed = (subagent: Subagent, item: Item): Subagent => ({
	...subagent,
	items: [...subagent.items, item],
});

// Its tool calls still running stop with it, and its report ends its transcript.
const ended = (
	subagent: Subagent,
	status: SubagentStatus,
	summary: string | null,
): Subagent => {
	const stopped = {
		...subagent,
		status,
		items: changeItems(subagent.items, Change.End()),
	};
	const last = stopped.items.at(-1);
	const isRepeated =
		last !== undefined && last._tag === "Reply" && last.text === summary;
	return summary === null || isRepeated
		? stopped
		: pushed(stopped, Item.Reply({ text: summary, state: "Done" }));
};

const toolStarted = (
	subagent: Subagent,
	call: ToolCallId,
	tool: ToolKind,
): Subagent =>
	hasCall(subagent.items, call)
		? {
				...subagent,
				items: subagent.items.map((item) =>
					item._tag === "Tool" && item.call === call
						? Item.Tool({ ...item, tool })
						: item,
				),
			}
		: pushed(
				subagent,
				Item.Tool({
					call,
					tool,
					state: ToolState.Running(),
					approval: Approval.NotAsked(),
				}),
			);

// Its transcript holds only finished items: a sub-agent's text arrives whole.
const did = Match.type<SubagentActivity>().pipe(
	Match.withReturnType<Update>(),
	Match.when(
		{ Message: Match.any },
		({ Message: { text } }) =>
			(subagent: Subagent) =>
				pushed(subagent, Item.Reply({ text, state: "Done" })),
	),
	Match.when(
		{ Reasoning: Match.any },
		({ Reasoning: { text } }) =>
			(subagent: Subagent) =>
				pushed(subagent, Item.Reasoning({ text, state: "Done" })),
	),
	Match.when(
		{ ToolCall: Match.any },
		({ ToolCall: { call, phase } }) =>
			(subagent: Subagent) =>
				"Finished" in phase
					? {
							...subagent,
							items: changeItems(
								subagent.items,
								Change.Finish({ call, outcome: phase.Finished.outcome }),
							),
						}
					: toolStarted(subagent, call, phase.Started.tool),
	),
	Match.exhaustive,
);

// A request joins its tool call's card; a request for a call never seen started gets a card of
// its own.
const asked =
	(call: ToolCallId, tool: ToolKind, approval: Approval): Update =>
	(subagent) =>
		hasCall(subagent.items, call)
			? {
					...subagent,
					items: changeItems(subagent.items, Change.Ask({ call, approval })),
				}
			: pushed(
					subagent,
					Item.Tool({ call, tool, state: ToolState.Running(), approval }),
				);

const update = Match.type<
	Exclude<SubagentEvent, { readonly Started: unknown }>
>().pipe(
	Match.withReturnType<Update>(),
	Match.when({ Activity: Match.any }, ({ Activity }) => did(Activity)),
	Match.when(
		{ Ended: Match.any },
		({ Ended: { outcome, summary } }) =>
			(subagent: Subagent) =>
				ended(subagent, SubagentStatus.Ended({ outcome }), summary),
	),
	Match.when(
		{ Kind: Match.any },
		({ Kind: { kind } }) =>
			(subagent: Subagent) => ({ ...subagent, kind }),
	),
	Match.when(
		{ ChecklistChanged: Match.any },
		({ ChecklistChanged: { checklist } }) =>
			(subagent: Subagent) => ({ ...subagent, checklist }),
	),
	Match.when({ RequestOpened: Match.any }, ({ RequestOpened: body }) =>
		asked(
			body.call,
			body.tool,
			openApproval(body.request, body.reason, body.for_session),
		),
	),
	Match.when(
		{ QuestionAsked: Match.any },
		({ QuestionAsked: { request, questions } }) =>
			(subagent: Subagent) =>
				pushed(
					subagent,
					Item.Question({ request, questions, state: RequestState.Open() }),
				),
	),
	Match.exhaustive,
);

// A sub-agent the agent resumes runs again under its id, after what it did before.
const started = (
	subagents: Subagents,
	id: SubagentId,
	call: ToolCallId,
	description: string,
): Subagents =>
	subagents.some((subagent) => subagent.id === id)
		? subagents.map((subagent) =>
				subagent.id === id
					? { ...subagent, status: SubagentStatus.Running() }
					: subagent,
			)
		: [
				...subagents,
				{
					id,
					call,
					description,
					kind: null,
					checklist: [],
					status: SubagentStatus.Running(),
					items: [],
				},
			];

// Events of a sub-agent never seen started are dropped.
const applySubagentEvent = (
	subagents: Subagents,
	id: SubagentId,
	event: SubagentEvent,
): Subagents => {
	if ("Started" in event) {
		return started(
			subagents,
			id,
			event.Started.call,
			event.Started.description,
		);
	}
	const apply = update(event);
	return subagents.map((subagent) =>
		subagent.id === id ? apply(subagent) : subagent,
	);
};

const changeSubagents = (subagents: Subagents, next: Change): Subagents =>
	subagents.map((subagent) => ({
		...subagent,
		items: changeItems(subagent.items, next),
	}));

const stopSubagents = (subagents: Subagents): Subagents =>
	subagents.map((subagent) =>
		subagent.status._tag === "Running"
			? ended(subagent, SubagentStatus.Stopped(), null)
			: subagent,
	);

export {
	applySubagentEvent,
	changeSubagents,
	stopSubagents,
	type Subagent,
	SubagentStatus,
};
