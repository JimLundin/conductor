// Events for the fold's worked examples, ported from the Rust HostView's tests.
import { isDelta } from "../link/feed.ts";
import { Update } from "../link/host.ts";
import type { RecordedFeed } from "../recorded-feeds.ts";
import type { EventKind, SessionId, SubagentEvent, ToolPhase } from "../schemas/index.ts";
import { applyUpdate, emptyHost, type Host } from "./host.ts";
import type { Session } from "./session.ts";
import { Approval, Item, RequestState, type ToolState } from "./transcript.ts";

type Numbered = readonly (readonly [number, EventKind])[];

const id = (session: number): SessionId => `0000000000000000000000000${session}`;

// Numbers each session's events as the host does: a delta repeats the last seq.
const foldEvents = (events: Numbered): Host =>
  events.reduce<readonly [Host, number]>(
    ([host, last], [session, kind]) => {
      const event = { seq: last + 1, session: id(session), kind };
      const seq = isDelta(event) ? last : event.seq;
      const frame = { Event: { ...event, seq } };
      return [applyUpdate(host, Update.Feed({ frame })), seq];
    },
    [emptyHost(), 0],
  )[0];

// What one connection of a recorded scenario's test client received, as its link reports it:
// its welcome, then its feed.
const received = (feed: RecordedFeed, connection: number): readonly Update[] =>
  feed
    .filter((recorded) => recorded.connection === connection)
    .flatMap((recorded): readonly Update[] => {
      if (recorded.stream === "hello" && "Welcome" in recorded.frame) {
        return [Update.Welcome({ welcome: recorded.frame.Welcome })];
      }
      return recorded.stream === "feed" ? [Update.Feed({ frame: recorded.frame })] : [];
    });

const foldUpdates = (updates: readonly Update[]): Host => updates.reduce(applyUpdate, emptyHost());

const sessionOf = (host: Host, session: SessionId): Session => {
  const found = host.sessions.get(session);
  if (found === undefined) {
    throw new Error(`no session ${session}`);
  }
  return found;
};

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
    request,
    call: `toolu_${request}`,
    tool: ls,
    reason: null,
    for_session: null,
  },
});

const delta = (text: string): EventKind => ({
  Agent: { AssistantDelta: { text } },
});

const reasoned = (text: string): EventKind => ({
  Agent: { ReasoningDelta: { text } },
});

const user = (text: string): EventKind => ({
  UserMessage: { text, by: "phone" },
});

const exited = (status: string): EventKind => ({
  SessionClosed: { reason: { Exited: { status } } },
});

const question = (request: number): EventKind => ({
  QuestionAsked: {
    request,
    questions: [
      {
        header: "Color",
        text: "Which color?",
        options: [{ label: "Blue", description: "Cool" }],
        selection: "One",
      },
    ],
  },
});

// An `ls` card that asked for approval.
const asked = (call: string, request: number, state: ToolState, approval: RequestState): Item =>
  Item.Tool({
    call,
    tool: ls,
    state,
    approval: Approval.Asked({
      request,
      reason: null,
      forSession: null,
      state: approval,
    }),
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
