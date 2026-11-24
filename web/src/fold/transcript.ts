// A transcript's items, and the changes later events make to earlier ones.
import { Data } from "effect";
import type {
	ClientName,
	CloseReason,
	Finished,
	Question,
	RequestId,
	Resolution,
	ToolCallId,
	ToolKind,
	ToolOutcome,
	TurnOutcome,
} from "../schemas/index.ts";

type Unit = Record<never, never>;

// A reply or reasoning streams until its final text arrives or something else follows it.
type ReplyState = "Streaming" | "Done";

type ToolState = Data.TaggedEnum<{
	readonly Running: Unit;
	readonly Finished: { readonly outcome: ToolOutcome };
	// The turn or the session ended before the call finished.
	readonly Stopped: Unit;
}>;

const ToolState = Data.taggedEnum<ToolState>();

type RequestState = Data.TaggedEnum<{
	readonly Open: Unit;
	readonly Resolved: { readonly resolution: Resolution };
}>;

const RequestState = Data.taggedEnum<RequestState>();

type Approval = Data.TaggedEnum<{
	readonly NotAsked: Unit;
	readonly Asked: {
		readonly request: RequestId;
		// The agent's own words for why it asks.
		readonly reason: string | null;
		// What approving for the session allows, when the agent offers it.
		readonly forSession: string | null;
		readonly state: RequestState;
	};
}>;

const Approval = Data.taggedEnum<Approval>();

type Item = Data.TaggedEnum<{
	readonly User: { readonly text: string; readonly by: ClientName };
	readonly Reply: { readonly text: string; readonly state: ReplyState };
	// The agent's reasoning, as its provider summarizes it.
	readonly Reasoning: { readonly text: string; readonly state: ReplyState };
	// A tool card: the call, with the approval it asked for.
	readonly Tool: {
		readonly call: ToolCallId;
		readonly tool: ToolKind;
		readonly state: ToolState;
		readonly approval: Approval;
	};
	readonly Question: {
		readonly request: RequestId;
		readonly questions: readonly Question[];
		readonly state: RequestState;
	};
	readonly TurnCompleted: { readonly outcome: TurnOutcome };
	// The agent started again after it had closed for this reason.
	readonly Continued: { readonly reason: CloseReason };
	readonly ModelChanged: { readonly reason: string };
	// Finishing the session on its issue began.
	readonly Finishing: Unit;
	readonly Finished: { readonly finished: Finished };
	readonly Error: { readonly message: string };
}>;

const Item = Data.taggedEnum<Item>();

// An in-place change to earlier items: asking, resolving and finishing are one map over them.
type Change = Data.TaggedEnum<{
	readonly Ask: { readonly call: ToolCallId; readonly approval: Approval };
	readonly Resolve: {
		readonly request: RequestId;
		readonly resolution: Resolution;
	};
	readonly Finish: { readonly call: ToolCallId; readonly outcome: ToolOutcome };
	// The host ended the turn or the session: nothing still running will finish, and nothing
	// still open will be answered.
	readonly End: Unit;
}>;

const Change = Data.taggedEnum<Change>();

const requestChanged = (
	state: RequestState,
	request: RequestId,
	next: Change,
): RequestState => {
	if (next._tag === "Resolve" && next.request === request) {
		return RequestState.Resolved({ resolution: next.resolution });
	}
	if (next._tag === "End" && state._tag === "Open") {
		return RequestState.Resolved({ resolution: "Withdrawn" });
	}
	return state;
};

const toolChanged = (
	state: ToolState,
	call: ToolCallId,
	next: Change,
): ToolState => {
	if (next._tag === "Finish" && next.call === call) {
		return ToolState.Finished({ outcome: next.outcome });
	}
	if (next._tag === "End" && state._tag === "Running") {
		return ToolState.Stopped();
	}
	return state;
};

const approvalChanged = (
	approval: Approval,
	call: ToolCallId,
	next: Change,
): Approval => {
	if (next._tag === "Ask" && next.call === call) {
		return next.approval;
	}
	if (approval._tag === "Asked") {
		return Approval.Asked({
			...approval,
			state: requestChanged(approval.state, approval.request, next),
		});
	}
	return approval;
};

const itemChanged =
	(next: Change) =>
	(item: Item): Item => {
		if (item._tag === "Tool") {
			return Item.Tool({
				...item,
				state: toolChanged(item.state, item.call, next),
				approval: approvalChanged(item.approval, item.call, next),
			});
		}
		if (item._tag === "Question") {
			return Item.Question({
				...item,
				state: requestChanged(item.state, item.request, next),
			});
		}
		return item;
	};

const changeItems = (items: readonly Item[], next: Change): readonly Item[] =>
	items.map(itemChanged(next));

// The item, done streaming: something else follows it.
const settled = (item: Item): Item => {
	if (item._tag === "Reply") {
		return Item.Reply({ text: item.text, state: "Done" });
	}
	if (item._tag === "Reasoning") {
		return Item.Reasoning({ text: item.text, state: "Done" });
	}
	return item;
};

// Settles the last item.
const settle = (items: readonly Item[]): readonly Item[] =>
	items.map((item, index) =>
		index === items.length - 1 ? settled(item) : item,
	);

type Streamed = "Reply" | "Reasoning";

// A delta joins the reply or reasoning still streaming; the final text replaces it, so a client
// that missed the deltas shows the same text.
const streamedItems = (
	items: readonly Item[],
	tag: Streamed,
	text: string,
	state: ReplyState,
): readonly Item[] => {
	const last = items.at(-1);
	const streaming =
		last !== undefined && last._tag === tag && last.state === "Streaming"
			? last.text
			: undefined;
	const earlier = streaming === undefined ? settle(items) : items.slice(0, -1);
	const joined = state === "Streaming" ? `${streaming ?? ""}${text}` : text;
	const item =
		tag === "Reply"
			? Item.Reply({ text: joined, state })
			: Item.Reasoning({ text: joined, state });
	return [...earlier, item];
};

const hasCall = (items: readonly Item[], call: ToolCallId): boolean =>
	items.some((item) => item._tag === "Tool" && item.call === call);

const openApproval = (
	request: RequestId,
	reason: string | null,
	forSession: string | null,
): Approval =>
	Approval.Asked({ request, reason, forSession, state: RequestState.Open() });

const isRequestOpen = (item: Item): boolean =>
	(item._tag === "Tool" &&
		item.approval._tag === "Asked" &&
		item.approval.state._tag === "Open") ||
	(item._tag === "Question" && item.state._tag === "Open");

export {
	Approval,
	Change,
	changeItems,
	hasCall,
	isRequestOpen,
	Item,
	openApproval,
	type ReplyState,
	RequestState,
	settle,
	type Streamed,
	streamedItems,
	ToolState,
	type Unit,
};
