import { Approval, Item, ToolState } from "@conductor/core/fold/transcript";
import {
  asked,
  created,
  delta,
  exited,
  finished,
  foldEvents,
  id,
  ls,
  opened,
  reasoned,
  sessionOf,
  started,
  withdrawn,
} from "#ui/test/examples.ts";
import { describe, expect, it } from "@effect/vitest";
import { directories, recent } from "@conductor/core/fold/host";
import { SessionStatus } from "@conductor/core/fold/session";
import { openRequests } from "@conductor/core/fold/offers";

describe("turn and session endings", () => {
  it("the end of a turn or a session stops its tools and withdraws its requests", () => {
    expect.hasAssertions();
    const host = foldEvents([
      [1, created("/a")],
      [1, started("toolu_1")],
      [1, opened(1)],
      [1, started("toolu_2")],
      [1, finished("toolu_2")],
      [1, { TurnCompleted: { outcome: "Interrupted" } }],
      [2, created("/b")],
      [2, opened(3)],
      [2, exited("1")],
    ]);
    expect(sessionOf(host, id(1)).items).toStrictEqual([
      asked({ approval: withdrawn, call: "toolu_1", request: 1, state: ToolState.Stopped() }),
      Item.Tool({
        approval: Approval.NotAsked(),
        call: "toolu_2",
        state: ToolState.Finished({ outcome: "Succeeded" }),
        tool: ls,
      }),
      Item.TurnCompleted({ outcome: "Interrupted" }),
    ]);
    expect(sessionOf(host, id(2)).items).toStrictEqual([
      asked({ approval: withdrawn, call: "toolu_3", request: 3, state: ToolState.Stopped() }),
    ]);
    const first = sessionOf(host, id(1));
    const second = sessionOf(host, id(2));
    expect(openRequests(first)).toStrictEqual([]);
    expect(openRequests(second)).toStrictEqual([]);
  });
});

describe("recent activity", () => {
  it("the list puts the latest activity first and suggests each directory once", () => {
    expect.hasAssertions();
    const host = foldEvents([
      [1, created("/a")],
      [2, created("/b")],
      [3, created("/a")],
      [2, "SessionReady"],
      [1, delta("x")],
    ]);
    expect(recent(host)).toStrictEqual([id(2), id(3), id(1)]);
    expect(directories(host)).toStrictEqual(["/b", "/a"]);
  });
});

describe("session removal", () => {
  it("a removed session leaves the host with its later events", () => {
    expect.hasAssertions();
    const host = foldEvents([
      [1, created("/a")],
      [2, created("/b")],
      [1, { SessionRemoved: { kept: null } }],
      [1, "SessionReady"],
    ]);
    expect(recent(host)).toStrictEqual([id(2)]);
  });
});

describe("continued reasoning", () => {
  it("reasoning streams into one item that its text replaces, and a continue shows its close", () => {
    expect.hasAssertions();
    const host = foldEvents([
      [1, created("/a")],
      [1, "SessionReady"],
      [1, reasoned("Look")],
      [1, reasoned("ing")],
      [1, { Agent: { Reasoning: { text: "Looking." } } }],
      [1, reasoned("Again")],
      [1, delta("Hi")],
      [1, exited("9")],
      [1, "SessionReady"],
    ]);
    expect(sessionOf(host, id(1)).items).toStrictEqual([
      Item.Reasoning({ state: "Done", text: "Looking." }),
      Item.Reasoning({ state: "Done", text: "Again" }),
      Item.Reply({ state: "Done", text: "Hi" }),
      Item.Continued({ reason: { Exited: { status: "9" } } }),
    ]);
    expect(sessionOf(host, id(1)).status).toStrictEqual(SessionStatus.Idle());
  });
});

describe("streamed deltas", () => {
  it("a reply and reasoning append every delta", () => {
    expect.hasAssertions();
    const host = foldEvents([
      [1, created("/a")],
      [1, reasoned("One")],
      [1, reasoned(" two")],
      [1, reasoned(" three")],
      [1, delta("Four")],
      [1, delta(" five")],
      [1, delta(" six")],
    ]);
    expect(sessionOf(host, id(1)).items).toStrictEqual([
      Item.Reasoning({ state: "Done", text: "One two three" }),
      Item.Reply({ state: "Streaming", text: "Four five six" }),
    ]);
  });
});

describe("closed streaming", () => {
  it("a close settles the reasoning or reply that was streaming", () => {
    expect.hasAssertions();
    const host = foldEvents([
      [1, created("/a")],
      [1, "SessionReady"],
      [1, reasoned("Look")],
      [1, exited("9")],
      [2, created("/b")],
      [2, "SessionReady"],
      [2, delta("Hel")],
      [2, exited("9")],
    ]);
    expect(sessionOf(host, id(1)).items).toStrictEqual([
      Item.Reasoning({ state: "Done", text: "Look" }),
    ]);
    expect(sessionOf(host, id(2)).items).toStrictEqual([
      Item.Reply({ state: "Done", text: "Hel" }),
    ]);
  });
});

describe("model changes", () => {
  it("a model change shows in the header and the transcript", () => {
    expect.hasAssertions();
    const reason = "Model changed to gpt: small is not offered";
    const host = foldEvents([
      [1, created("/a")],
      [1, "SessionReady"],
      [1, exited("9")],
      [1, { ModelChanged: { effort: null, model: "gpt", reason } }],
    ]);
    expect(sessionOf(host, id(1)).model).toBe("gpt");
    expect(sessionOf(host, id(1)).items).toStrictEqual([Item.ModelChanged({ reason })]);
  });
});

describe("checklists and effort", () => {
  it("each change replaces the checklist, and the effort follows its last change", () => {
    expect.hasAssertions();
    const later = [{ status: "Completed", text: "write a.txt" }] as const;
    const host = foldEvents([
      [1, created("/a")],
      [1, "SessionReady"],
      [
        1,
        {
          ChecklistChanged: {
            checklist: [
              { status: "InProgress", text: "write a.txt" },
              { status: "Pending", text: "write b.txt" },
            ],
          },
        },
      ],
      [1, { ChecklistChanged: { checklist: later } }],
      [1, { EffortChanged: { effort: "Low" } }],
    ]);
    expect(sessionOf(host, id(1)).checklist).toStrictEqual(later);
    expect(sessionOf(host, id(1)).effort).toBe("Low");
    expect(sessionOf(host, id(1)).items).toStrictEqual([]);
  });
});
