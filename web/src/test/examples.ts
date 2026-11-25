import { Approval, Item, RequestState } from "@conductor/core/fold/transcript";
import { Array as Arr, Option } from "effect";
import type { EventKind, SessionId, SubagentEvent, ToolPhase } from "@conductor/core/schemas/index";
import { Update, isDelta } from "@conductor/core/link";
import { applyUpdate, emptyHost } from "@conductor/core/fold/host";
import type { Host } from "@conductor/core/fold/host";
import type { RecordedFeed } from "#ui/test/recorded-feeds.ts";
import type { Session } from "@conductor/core/fold/session";
import type { ToolState } from "@conductor/core/fold/transcript";

type Numbered = ReadonlyArray<readonly [number, EventKind]>;

const id = (session: number): SessionId => `0000000000000000000000000${session}`;

const foldEvents = (events: Numbered): Host =>
  Arr.reduce<readonly [number, EventKind], readonly [Host, number]>(
    events,
    [emptyHost(), 0],
    ([host, last], [session, kind]) => {
      const event = { kind, seq: last + 1, session: id(session) };
      const seq = isDelta(event) ? last : event.seq;
      const frame = { Event: { ...event, seq } };
      return [applyUpdate(host, Update.Feed({ frame })), seq];
    },
  )[0];

const received = (feed: RecordedFeed, connection: number): ReadonlyArray<Update> =>
  feed
    .filter((recorded) => recorded.connection === connection)
    .flatMap((recorded): ReadonlyArray<Update> => {
      if (recorded.stream === "hello" && "Welcome" in recorded.frame) {
        return [Update.Welcome({ welcome: recorded.frame.Welcome })];
      }
      return recorded.stream === "feed" ? [Update.Feed({ frame: recorded.frame })] : [];
    });

const foldUpdates = (updates: ReadonlyArray<Update>): Host =>
  Arr.reduce(updates, emptyHost(), applyUpdate);

const sessionOf = (host: Host, session: SessionId): Session =>
  Option.getOrThrow(Option.fromNullishOr(host.sessions.get(session)));

const created = (directory: string): EventKind => ({
  SessionCreated: {
    by: "desk",
    directory,
    effort: null,
    instance: { account: "Own", provider: "claude" },
    model: null,
  },
});

const ls = { Command: { command: "ls" } } as const;

const started = (call: string): EventKind => ({
  Agent: { ToolCall: { call, phase: { Started: { tool: ls } } } },
});

const finished = (call: string): EventKind => ({
  Agent: { ToolCall: { call, phase: { Finished: { outcome: "Succeeded" } } } },
});

const opened = (request: number): EventKind => ({
  RequestOpened: {
    call: `toolu_${request}`,
    for_session: null,
    reason: null,
    request,
    tool: ls,
  },
});

const delta = (text: string): EventKind => ({
  Agent: { AssistantDelta: { text } },
});

const reasoned = (text: string): EventKind => ({
  Agent: { ReasoningDelta: { text } },
});

const user = (text: string): EventKind => ({
  UserMessage: { by: "phone", text },
});

const exited = (status: string): EventKind => ({
  SessionClosed: { reason: { Exited: { status } } },
});

const question = (request: number): EventKind => ({
  QuestionAsked: {
    questions: [
      {
        header: "Color",
        options: [{ description: "Cool", label: "Blue" }],
        selection: "One",
        text: "Which color?",
      },
    ],
    request,
  },
});

interface Asked {
  readonly approval: RequestState;
  readonly call: string;
  readonly request: number;
  readonly state: ToolState;
}

const asked = ({ approval, call, request, state }: Asked): Item =>
  Item.Tool({
    approval: Approval.Asked({
      forSession: undefined,
      reason: undefined,
      request,
      state: approval,
    }),
    call,
    state,
    tool: ls,
  });

const withdrawn = RequestState.Resolved({ resolution: "Withdrawn" });

const subagent = (agent: string, event: SubagentEvent): EventKind => ({
  Subagent: { agent, event },
});

const subagentStarted = (agent: string, call: string): EventKind =>
  subagent(agent, { Started: { call, description: `look for ${agent}` } });

const subagentTool = (agent: string, call: string, phase: ToolPhase): EventKind =>
  subagent(agent, { Activity: { ToolCall: { call, phase } } });

export {
  asked,
  created,
  delta,
  exited,
  finished,
  foldEvents,
  foldUpdates,
  id,
  ls,
  type Numbered,
  opened,
  question,
  reasoned,
  received,
  sessionOf,
  started,
  subagent,
  subagentStarted,
  subagentTool,
  user,
  withdrawn,
};
