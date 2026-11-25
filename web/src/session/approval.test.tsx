import type { Decision, EventKind } from "@conductor/core/schemas/index";
import { answer, answersOf, decisions, notice } from "@conductor/core/session/commands";
import { askedOf, offered } from "#ui/test/session-requests.ts";
import { button, view } from "#ui/test/session.tsx";
import { describe, expect, it } from "@effect/vitest";
import { ls, question, started, user } from "#ui/test/examples.ts";
import { onSession, ready, session, turnThrough } from "#ui/test/session-events.ts";

const askedForSession = {
  RequestOpened: {
    call: "toolu_1",
    for_session: "Bash(ls)",
    reason: "to see the files",
    request: 1,
    tool: ls,
  },
} as const satisfies EventKind;

const approve = (label: string): ReadonlyArray<EventKind> => [
  ...ready(),
  user(label),
  started("toolu_1"),
  askedForSession,
];

const decided = (
  events: ReadonlyArray<EventKind>,
  decision: Decision,
): ReadonlyArray<EventKind> => [
  ...events,
  { RequestResolved: { request: 1, resolution: { Decided: { by: "phone", decision } } } },
];

describe("an approval", () => {
  it("sends the decision to the selected request", () => {
    expect.hasAssertions();
    const host = turnThrough(8);
    const asked = askedOf(host);
    const choices = decisions(session, asked);
    expect(offered(choices, "Approve")).toStrictEqual({
      Session: { Resolve: { decision: "Approve", request: 1, session } },
    });
  });

  it("shows who approved after the host resolves the request", () => {
    expect.hasAssertions();
    const finished = turnThrough(13);
    const html = view(finished);
    expect(html).toContain("Approved by test-client");
    expect(html).not.toContain("Waiting for approval");
  });
});

describe("session approval choices", () => {
  it.each([
    ["Approve for session", "ApproveForSession", "Approved for the session by phone"],
    ["Deny", "Deny", "Denied by phone"],
  ] as const)("offers %s and shows the host's decision", (label, decision: Decision, shown) => {
    expect.hasAssertions();
    const asking = approve("look");
    const before = onSession(asking);
    const choices = decisions(session, askedOf(before));
    const html = view(before);
    expect(html).toContain(button(label));
    expect(html).toContain("For the session: Bash(ls)");
    expect(offered(choices, label)).toStrictEqual({
      Session: { Resolve: { decision, request: 1, session } },
    });
    const after = onSession(decided(asking, decision));
    expect(view(after)).toContain(shown);
  });
});

describe("a refused command", () => {
  it.each([
    [
      { Session: { Refused: { AlreadyResolved: { by: "test-client" } } } },
      "The host refused: Already resolved: test-client.",
    ],
    [{ Session: { Refused: "TurnInProgress" } }, "The host refused: Turn in progress."],
  ] as const)("explains the host's refusal", (result, message) => {
    expect.hasAssertions();
    expect(notice(result)).toBe(message);
  });

  it("adds no notice for an accepted command", () => {
    expect.hasAssertions();
    expect(notice({ Session: "Accepted" })).toBeUndefined();
  });
});

describe("a session question", () => {
  it("sends the chosen options and shows the host's answer", () => {
    expect.hasAssertions();
    const asking = [...ready(), user("paint it"), question(1)];
    const before = onSession(asking);
    const html = view(before);
    const picked = answersOf([{ labels: ["Blue"], typed: "" }]);
    expect(html).toContain("<legend>Color</legend><p>Which color?</p>");
    expect(html).toContain('title="Cool" type="button">Blue</button>');
    expect(answer(session, 1, picked)).toStrictEqual({
      Session: { Answer: { answers: [["Blue"]], request: 1, session } },
    });
    const after = onSession([
      ...asking,
      {
        RequestResolved: {
          request: 1,
          resolution: { Answered: { answers: [["Blue"]], by: "phone" } },
        },
      },
    ]);
    expect(view(after)).toContain("Answered by phone: Blue");
  });

  it("keeps selected options before any typed alternative", () => {
    expect.hasAssertions();
    expect(
      answersOf([
        { labels: ["Blue", "Red"], typed: "  teal " },
        { labels: [], typed: "" },
      ]),
    ).toStrictEqual([["Blue", "Red", "teal"], []]);
  });
});
