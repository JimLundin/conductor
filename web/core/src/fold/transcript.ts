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
} from "#core/schemas/index";
import { Data } from "effect";
import type { Unit } from "#core/data";
// A transcript's items, and the changes later events make to earlier ones.

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
    readonly reason: string | undefined;
    // What approving for the session allows, when the agent offers it.
    readonly forSession: string | undefined;
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
    readonly questions: ReadonlyArray<Question>;
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
  // Still open will be answered.
  readonly End: Unit;
}>;

const Change = Data.taggedEnum<Change>();

const requestChanged = (state: RequestState, request: RequestId, next: Change): RequestState => {
  if (Change.$is("Resolve")(next) && next.request === request) {
    return RequestState.Resolved({ resolution: next.resolution });
  }
  if (Change.$is("End")(next) && RequestState.$is("Open")(state)) {
    return RequestState.Resolved({ resolution: "Withdrawn" });
  }
  return state;
};

const toolChanged = (state: ToolState, call: ToolCallId, next: Change): ToolState => {
  if (Change.$is("Finish")(next) && next.call === call) {
    return ToolState.Finished({ outcome: next.outcome });
  }
  if (Change.$is("End")(next) && ToolState.$is("Running")(state)) {
    return ToolState.Stopped();
  }
  return state;
};

const approvalChanged = (approval: Approval, call: ToolCallId, next: Change): Approval => {
  if (Change.$is("Ask")(next) && next.call === call) {
    return next.approval;
  }
  if (Approval.$is("Asked")(approval)) {
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
    if (Item.$is("Tool")(item)) {
      return Item.Tool({
        ...item,
        approval: approvalChanged(item.approval, item.call, next),
        state: toolChanged(item.state, item.call, next),
      });
    }
    if (Item.$is("Question")(item)) {
      return Item.Question({
        ...item,
        state: requestChanged(item.state, item.request, next),
      });
    }
    return item;
  };

const changeItems = (items: ReadonlyArray<Item>, next: Change): ReadonlyArray<Item> =>
  items.map(itemChanged(next));

// The item, done streaming: something else follows it.
const settled = (item: Item): Item => {
  if (Item.$is("Reply")(item)) {
    return Item.Reply({ state: "Done", text: item.text });
  }
  if (Item.$is("Reasoning")(item)) {
    return Item.Reasoning({ state: "Done", text: item.text });
  }
  return item;
};

const firstItem = 0;
const lastItem = -1;
const lastOffset = 1;

// Settles the last item.
const settle = (items: ReadonlyArray<Item>): ReadonlyArray<Item> =>
  items.map((item, index) => (index === items.length - lastOffset ? settled(item) : item));

type Streamed = "Reply" | "Reasoning";

// A delta joins the reply or reasoning still streaming; the final text replaces it, so a client
// That missed the deltas shows the same text.
interface Fragment {
  readonly state: ReplyState;
  readonly tag: Streamed;
  readonly text: string;
}

const streamedItems = (
  items: ReadonlyArray<Item>,
  { tag, text, state }: Fragment,
): ReadonlyArray<Item> => {
  const last = items.at(lastItem);
  const streaming =
    last !== undefined && Item.$is(tag)(last) && last.state === "Streaming" ? last.text : undefined;
  const earlier = streaming === undefined ? settle(items) : items.slice(firstItem, lastItem);
  const joined = state === "Streaming" ? `${streaming ?? ""}${text}` : text;
  const item =
    tag === "Reply" ? Item.Reply({ state, text: joined }) : Item.Reasoning({ state, text: joined });
  return [...earlier, item];
};

const hasCall = (items: ReadonlyArray<Item>, call: ToolCallId): boolean =>
  items.some((item) => Item.$is("Tool")(item) && item.call === call);

const openApproval = (
  request: RequestId,
  reason: string | null,
  forSession: string | null,
): Approval =>
  Approval.Asked({
    forSession: forSession ?? undefined,
    reason: reason ?? undefined,
    request,
    state: RequestState.Open(),
  });

const isRequestOpen = (item: Item): boolean =>
  (Item.$is("Tool")(item) &&
    Approval.$is("Asked")(item.approval) &&
    RequestState.$is("Open")(item.approval.state)) ||
  (Item.$is("Question")(item) && RequestState.$is("Open")(item.state));

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
};
