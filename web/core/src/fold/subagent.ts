import {
  Approval,
  Change,
  Item,
  RequestState,
  ToolState,
  changeItems,
  hasCall,
  openApproval,
} from "#core/fold/transcript";
import type {
  ChecklistEntry,
  SubagentActivity,
  SubagentEvent,
  SubagentId,
  SubagentOutcome,
  ToolCallId,
  ToolKind,
} from "#core/schemas/index";
import { Data, Match } from "effect";
import type { Unit } from "#core/data";
// A session's sub-agents: children with no session of their own, whose events are its.

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
  readonly kind: string | undefined;
  readonly checklist: ReadonlyArray<ChecklistEntry>;
  readonly status: SubagentStatus;
  // Its own transcript, whose last reply is its report.
  readonly items: ReadonlyArray<Item>;
}

type Subagents = ReadonlyArray<Subagent>;

type Update = (subagent: Subagent) => Subagent;

const lastItem = -1;

const pushed = (subagent: Subagent, item: Item): Subagent => ({
  ...subagent,
  items: [...subagent.items, item],
});

// Its tool calls still running stop with it, and its report ends its transcript.
const ended = (subagent: Subagent, status: SubagentStatus, summary?: string): Subagent => {
  const stopped = {
    ...subagent,
    items: changeItems(subagent.items, Change.End()),
    status,
  };
  const last = stopped.items.at(lastItem);
  const isRepeated = last !== undefined && Item.$is("Reply")(last) && last.text === summary;
  return summary === undefined || isRepeated
    ? stopped
    : pushed(stopped, Item.Reply({ state: "Done", text: summary }));
};

const toolStarted = (subagent: Subagent, call: ToolCallId, tool: ToolKind): Subagent =>
  hasCall(subagent.items, call)
    ? {
        ...subagent,
        items: subagent.items.map((item) =>
          Item.$is("Tool")(item) && item.call === call ? Item.Tool({ ...item, tool }) : item,
        ),
      }
    : pushed(
        subagent,
        Item.Tool({
          approval: Approval.NotAsked(),
          call,
          state: ToolState.Running(),
          tool,
        }),
      );

// Its transcript holds only finished items: a sub-agent's text arrives whole.
const did = Match.type<SubagentActivity>().pipe(
  Match.withReturnType<Update>(),
  Match.when(
    { Message: Match.any },
    ({ Message: { text } }) =>
      (subagent: Subagent): Subagent =>
        pushed(subagent, Item.Reply({ state: "Done", text })),
  ),
  Match.when(
    { Reasoning: Match.any },
    ({ Reasoning: { text } }) =>
      (subagent: Subagent): Subagent =>
        pushed(subagent, Item.Reasoning({ state: "Done", text })),
  ),
  Match.when(
    { ToolCall: Match.any },
    ({ ToolCall: { call, phase } }) =>
      (subagent: Subagent): Subagent =>
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
// Its own.
const asked =
  (call: ToolCallId, tool: ToolKind, approval: Approval): Update =>
  (subagent) =>
    hasCall(subagent.items, call)
      ? {
          ...subagent,
          items: changeItems(subagent.items, Change.Ask({ approval, call })),
        }
      : pushed(subagent, Item.Tool({ approval, call, state: ToolState.Running(), tool }));

const update = Match.type<Exclude<SubagentEvent, { readonly Started: unknown }>>().pipe(
  Match.withReturnType<Update>(),
  Match.when({ Activity: Match.any }, ({ Activity }) => did(Activity)),
  Match.when(
    { Ended: Match.any },
    ({ Ended: { outcome, summary } }) =>
      (subagent: Subagent): Subagent =>
        ended(subagent, SubagentStatus.Ended({ outcome }), summary ?? undefined),
  ),
  Match.when({ Kind: Match.any }, ({ Kind: { kind } }) => (subagent: Subagent): Subagent => ({
    ...subagent,
    kind: kind ?? undefined,
  })),
  Match.when(
    { ChecklistChanged: Match.any },
    ({ ChecklistChanged: { checklist } }) =>
      (subagent: Subagent): Subagent => ({ ...subagent, checklist }),
  ),
  Match.when({ RequestOpened: Match.any }, ({ RequestOpened: body }) =>
    asked(body.call, body.tool, openApproval(body.request, body.reason, body.for_session)),
  ),
  Match.when(
    { QuestionAsked: Match.any },
    ({ QuestionAsked: { request, questions } }) =>
      (subagent: Subagent): Subagent =>
        pushed(subagent, Item.Question({ questions, request, state: RequestState.Open() })),
  ),
  Match.exhaustive,
);

// A sub-agent the agent resumes runs again under its id, after what it did before.
const started = (
  subagents: Subagents,
  id: SubagentId,
  { call, description }: { readonly call: ToolCallId; readonly description: string },
): Subagents =>
  subagents.some((subagent) => subagent.id === id)
    ? subagents.map((subagent) =>
        subagent.id === id ? { ...subagent, status: SubagentStatus.Running() } : subagent,
      )
    : [
        ...subagents,
        {
          call,
          checklist: [],
          description,
          id,
          items: [],
          kind: undefined,
          status: SubagentStatus.Running(),
        },
      ];

// Events of a sub-agent never seen started are dropped.
const applySubagentEvent = (
  subagents: Subagents,
  id: SubagentId,
  event: SubagentEvent,
): Subagents => {
  if ("Started" in event) {
    return started(subagents, id, event.Started);
  }
  const apply = update(event);
  return subagents.map((subagent) => (subagent.id === id ? apply(subagent) : subagent));
};

const changeSubagents = (subagents: Subagents, next: Change): Subagents =>
  subagents.map((subagent) => ({
    ...subagent,
    items: changeItems(subagent.items, next),
  }));

const stopSubagents = (subagents: Subagents): Subagents =>
  subagents.map((subagent) =>
    SubagentStatus.$is("Running")(subagent.status)
      ? ended(subagent, SubagentStatus.Stopped())
      : subagent,
  );

export { applySubagentEvent, changeSubagents, stopSubagents, type Subagent, SubagentStatus };
