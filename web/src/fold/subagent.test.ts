import { Approval, Item, RequestState, ToolState } from "@conductor/core/fold/transcript";
import {
  asked,
  created,
  exited,
  foldEvents,
  id,
  ls,
  sessionOf,
  started,
  subagent,
  subagentStarted,
  subagentTool,
} from "#ui/test/examples.ts";
import { describe, expect, it } from "@effect/vitest";
import type { Numbered } from "#ui/test/examples.ts";
import type { Resolution } from "@conductor/core/schemas/index";
import { SubagentStatus } from "@conductor/core/fold/subagent";
import { openRequests } from "@conductor/core/fold/offers";

const approved = {
  Decided: { by: "phone", decision: "Approve" },
} as const satisfies Resolution;

// Two sub-agents: a asks to run a call, and b starts after it.
const asking = [
  [1, created("/a")],
  [1, "SessionReady"],
  [1, started("toolu_agent_a")],
  [1, subagentStarted("a", "toolu_agent_a")],
  [1, started("toolu_agent_b")],
  [1, subagentStarted("b", "toolu_agent_b")],
  [1, subagent("a", { Activity: { Reasoning: { text: "where?" } } })],
  [1, subagentTool("a", "toolu_a1", { Started: { tool: ls } })],
  [
    1,
    subagent("a", {
      RequestOpened: {
        call: "toolu_a1",
        for_session: null,
        reason: null,
        request: 1,
        tool: ls,
      },
    }),
  ],
] as const satisfies Numbered;

// The call is approved and a finishes with its report; b still runs a call when the session
// Closes.
const closing = [
  [1, { RequestResolved: { request: 1, resolution: approved } }],
  [1, subagentTool("a", "toolu_a1", { Finished: { outcome: "Succeeded" } })],
  [1, subagent("a", { Activity: { Message: { text: "found it" } } })],
  [1, subagent("a", { Ended: { outcome: "Finished", summary: "it is in /a" } })],
  [1, subagentTool("b", "toolu_b1", { Started: { tool: ls } })],
  [1, subagent("nobody", { Activity: { Message: { text: "lost" } } })],
  [1, exited("1")],
] as const satisfies Numbered;

describe("subagent approvals", () => {
  it("a sub-agent's request is its session's open request", () => {
    expect.hasAssertions();
    const host = foldEvents(asking);
    const session = sessionOf(host, id(1));
    expect(openRequests(session)).toStrictEqual([
      asked({
        approval: RequestState.Open(),
        call: "toolu_a1",
        request: 1,
        state: ToolState.Running(),
      }),
    ]);
  });
});

describe("subagent closure", () => {
  it("sub-agents fold under their session and stop when it closes", () => {
    expect.hasAssertions();
    const session = sessionOf(foldEvents([...asking, ...closing]), id(1));
    expect(session.subagents).toStrictEqual([
      {
        call: "toolu_agent_a",
        checklist: [],
        description: "look for a",
        id: "a",
        items: [
          Item.Reasoning({ state: "Done", text: "where?" }),
          asked({
            approval: RequestState.Resolved({ resolution: approved }),
            call: "toolu_a1",
            request: 1,
            state: ToolState.Finished({ outcome: "Succeeded" }),
          }),
          Item.Reply({ state: "Done", text: "found it" }),
          Item.Reply({ state: "Done", text: "it is in /a" }),
        ],
        kind: undefined,
        status: SubagentStatus.Ended({ outcome: "Finished" }),
      },
      {
        call: "toolu_agent_b",
        checklist: [],
        description: "look for b",
        id: "b",
        items: [
          Item.Tool({
            approval: Approval.NotAsked(),
            call: "toolu_b1",
            state: ToolState.Stopped(),
            tool: ls,
          }),
        ],
        kind: undefined,
        status: SubagentStatus.Stopped(),
      },
    ]);
    expect(session.items).toMatchObject([
      { call: "toolu_agent_a", state: ToolState.Stopped() },
      { call: "toolu_agent_b", state: ToolState.Stopped() },
    ]);
  });
});

describe("subagent tool identity", () => {
  it("a child's tool result doesn't finish a main tool call with the same id", () => {
    expect.hasAssertions();
    const session = sessionOf(
      foldEvents([
        [1, created("/a")],
        [1, "SessionReady"],
        [1, started("shared")],
        [1, subagentStarted("a", "spawn")],
        [1, subagentTool("a", "shared", { Started: { tool: ls } })],
        [1, subagentTool("a", "shared", { Finished: { outcome: "Succeeded" } })],
      ]),
      id(1),
    );
    expect(session.items.at(-1)).toStrictEqual(
      Item.Tool({
        approval: Approval.NotAsked(),
        call: "shared",
        state: ToolState.Running(),
        tool: ls,
      }),
    );
    expect(session.subagents[0]?.items.at(-1)).toStrictEqual(
      Item.Tool({
        approval: Approval.NotAsked(),
        call: "shared",
        state: ToolState.Finished({ outcome: "Succeeded" }),
        tool: ls,
      }),
    );
  });
});

describe("subagent resumption", () => {
  it("a resumed sub-agent runs again", () => {
    expect.hasAssertions();
    const session = sessionOf(
      foldEvents([
        [1, created("/a")],
        [1, "SessionReady"],
        [1, started("toolu_agent_a")],
        [1, subagentStarted("a", "toolu_agent_a")],
        [1, subagent("a", { Ended: { outcome: "Failed", summary: null } })],
        [1, subagentStarted("a", "toolu_agent_a")],
      ]),
      id(1),
    );
    expect(session.subagents.map((known) => known.status)).toStrictEqual([
      SubagentStatus.Running(),
    ]);
  });
});
