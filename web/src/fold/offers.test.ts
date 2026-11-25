import type { EventKind, Resolution } from "@conductor/core/schemas/index";
import {
  ListStatus,
  emptyTranscript,
  isWorking,
  listStatus,
  openRequests,
} from "@conductor/core/fold/offers";
import {
  created,
  delta,
  finished,
  foldEvents,
  id,
  opened,
  question,
  sessionOf,
  started,
  user,
  withdrawn,
} from "#ui/test/examples.ts";
import { describe, expect, it } from "@effect/vitest";
import { RequestState } from "@conductor/core/fold/transcript";

const cases = [
  [[], false],
  [[user("hi")], true],
  [[user("hi"), delta("Hel")], false],
  [[user("hi"), delta("Hel"), { Agent: { AssistantMessage: { text: "Hello" } } }], true],
  [[user("hi"), { Agent: { ReasoningDelta: { text: "Hm" } } }], false],
  [[user("hi"), started("toolu_1")], false],
  [[user("hi"), started("toolu_1"), finished("toolu_1")], true],
  [[user("hi"), { TurnCompleted: { outcome: "Completed" } }], false],
  [[user("hi"), started("toolu_1"), started("toolu_2"), finished("toolu_2"), opened(1)], false],
  [
    [
      user("hi"),
      started("toolu_1"),
      { Agent: { AssistantMessage: { text: "I need to write a file." } } },
      opened(1),
    ],
    false,
  ],
] as const satisfies ReadonlyArray<readonly [ReadonlyArray<EventKind>, boolean]>;

describe("working status", () => {
  it.each(cases)(
    "a running turn shows it is working until something live ends the transcript (%j)",
    (events, shown) => {
      expect.hasAssertions();
      const host = foldEvents(
        [created("/a"), "SessionReady" as const, ...events].map((kind) => [1, kind] as const),
      );
      const session = sessionOf(host, id(1));
      expect(isWorking(session)).toBe(shown);
    },
  );
});

describe("session list status", () => {
  it("the list shows the open requests in place of the session's status", () => {
    expect.hasAssertions();
    const host = foldEvents([
      [1, created("/a")],
      [1, opened(1)],
      [1, opened(2)],
      [1, opened(3)],
      [1, { RequestResolved: { request: 2, resolution: "Withdrawn" } }],
      [2, created("/b")],
    ]);
    const first = sessionOf(host, id(1));
    const second = sessionOf(host, id(2));
    expect(listStatus(first)).toStrictEqual(ListStatus.Requests({ count: 2 }));
    expect(listStatus(second)).toStrictEqual(ListStatus.Session({ status: "Starting" }));
    expect(emptyTranscript(second)).toBe("Starting");
    expect(emptyTranscript(first)).toBeUndefined();
  });
});

describe("question lifecycle", () => {
  it("a question waits on a person until answered or the turn ends", () => {
    expect.hasAssertions();
    const answered: Resolution = {
      Answered: { answers: [["Blue"]], by: "phone" },
    };
    const open = foldEvents([
      [1, created("/a")],
      [1, "SessionReady"],
      [1, user("ask me")],
      [1, question(1)],
      [1, question(2)],
      [1, opened(3)],
    ]);
    const pending = sessionOf(open, id(1));
    expect(openRequests(pending)).toHaveLength(3);
    expect(isWorking(pending)).toBe(false);
    const later = foldEvents([
      [1, created("/a")],
      [1, question(1)],
      [1, question(2)],
      [1, { RequestResolved: { request: 1, resolution: answered } }],
      [1, { TurnCompleted: { outcome: "Interrupted" } }],
    ]);
    const settled = sessionOf(later, id(1));
    expect(settled.items).toMatchObject([
      { request: 1, state: RequestState.Resolved({ resolution: answered }) },
      { request: 2, state: withdrawn },
      { outcome: "Interrupted" },
    ]);
    expect(openRequests(settled)).toStrictEqual([]);
  });
});
