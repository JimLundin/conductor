import { describe, expect, it } from "@effect/vitest";
import type { Resolution } from "@conductor/core/schemas/index";
import {
  asked,
  created,
  exited,
  foldEvents,
  id,
  ls,
  type Numbered,
  sessionOf,
  started,
  subagent,
  subagentStarted,
  subagentTool,
} from "#ui/test/examples.ts";
import { openRequests } from "@conductor/core/fold/offers";
import { SubagentStatus } from "@conductor/core/fold/subagent";
import { Approval, Item, RequestState, ToolState } from "@conductor/core/fold/transcript";

describe("subagent", () => {
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
          request: 1,
          call: "toolu_a1",
          tool: ls,
          reason: null,
          for_session: null,
        },
      }),
    ],
  ] as const satisfies Numbered;

  // The call is approved and a finishes with its report; b still runs a call when the session
  // closes.
  const closing = [
    [1, { RequestResolved: { request: 1, resolution: approved } }],
    [1, subagentTool("a", "toolu_a1", { Finished: { outcome: "Succeeded" } })],
    [1, subagent("a", { Activity: { Message: { text: "found it" } } })],
    [1, subagent("a", { Ended: { outcome: "Finished", summary: "it is in /a" } })],
    [1, subagentTool("b", "toolu_b1", { Started: { tool: ls } })],
    [1, subagent("nobody", { Activity: { Message: { text: "lost" } } })],
    [1, exited("1")],
  ] as const satisfies Numbered;

  it("a sub-agent's request is its session's open request", () => {
    expect.hasAssertions();
    expect(openRequests(sessionOf(foldEvents(asking), id(1)))).toStrictEqual([
      asked("toolu_a1", 1, ToolState.Running(), RequestState.Open()),
    ]);
  });

  it("sub-agents fold under their session and stop when it closes", () => {
    expect.hasAssertions();
    const session = sessionOf(foldEvents([...asking, ...closing]), id(1));
    expect(session.subagents).toStrictEqual([
      {
        id: "a",
        call: "toolu_agent_a",
        description: "look for a",
        kind: null,
        checklist: [],
        status: SubagentStatus.Ended({ outcome: "Finished" }),
        items: [
          Item.Reasoning({ text: "where?", state: "Done" }),
          asked(
            "toolu_a1",
            1,
            ToolState.Finished({ outcome: "Succeeded" }),
            RequestState.Resolved({ resolution: approved }),
          ),
          Item.Reply({ text: "found it", state: "Done" }),
          Item.Reply({ text: "it is in /a", state: "Done" }),
        ],
      },
      {
        id: "b",
        call: "toolu_agent_b",
        description: "look for b",
        kind: null,
        checklist: [],
        status: SubagentStatus.Stopped(),
        items: [
          Item.Tool({
            call: "toolu_b1",
            tool: ls,
            state: ToolState.Stopped(),
            approval: Approval.NotAsked(),
          }),
        ],
      },
    ]);
    expect(
      session.items.map((item) => (item._tag === "Tool" ? [item.call, item.state] : null)),
    ).toStrictEqual([
      ["toolu_agent_a", ToolState.Stopped()],
      ["toolu_agent_b", ToolState.Stopped()],
    ]);
  });

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
        call: "shared",
        tool: ls,
        state: ToolState.Running(),
        approval: Approval.NotAsked(),
      }),
    );
    expect(session.subagents[0]?.items.at(-1)).toStrictEqual(
      Item.Tool({
        call: "shared",
        tool: ls,
        state: ToolState.Finished({ outcome: "Succeeded" }),
        approval: Approval.NotAsked(),
      }),
    );
  });

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
