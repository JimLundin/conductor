import { button, shownSession, view } from "#ui/test/session.tsx";
import { created, exited, started, user } from "#ui/test/examples.ts";
import { describe, expect, it } from "@effect/vitest";
import { directory, onSession, ready, session } from "#ui/test/session-events.ts";
import { offered } from "#ui/test/session-requests.ts";

describe("interrupting a turn", () => {
  it("offers Stop while running and shows the interrupted result", () => {
    expect.hasAssertions();
    const running = [...ready(), user("look"), started("toolu_1")];
    const before = shownSession(onSession(running));
    expect(before.html).toContain(button("Stop"));
    expect(offered(before.choices, "Stop")).toStrictEqual({ Session: { Interrupt: { session } } });
    const after = onSession([...running, { TurnCompleted: { outcome: "Interrupted" } }]);
    const html = view(after);
    expect(html).toContain('<p class="marker stopped">Interrupted</p>');
    expect(html).toContain('data-status="Stopped"');
  });
});

describe("continuing a session", () => {
  it("offers Continue once closed and shows the continued conversation", () => {
    expect.hasAssertions();
    const closed = [...ready(), user("look"), exited("1")];
    const before = shownSession(onSession(closed));
    expect(before.html).toContain(button("Continue"));
    expect(before.html).toContain("Closed: it exited with 1");
    expect(offered(before.choices, "Continue")).toStrictEqual({
      Session: { Continue: { session } },
    });
    const after = onSession([...closed, "SessionReady"]);
    expect(view(after)).toContain("Continued after it exited with 1");
  });
});

describe("finishing a session", () => {
  it("offers Finish on an issue and shows the landing result", () => {
    expect.hasAssertions();
    const onIssue = [
      created(directory),
      { OnIssue: { issue: "local~0twtnsfs" } },
      "SessionReady",
    ] as const;
    const before = shownSession(onSession(onIssue));
    expect(before.html).toContain(button("Finish"));
    expect(offered(before.choices, "Finish")).toStrictEqual({ Session: { Finish: { session } } });
    const after = onSession([
      ...onIssue,
      "FinishStarted",
      {
        SessionFinished: { finished: { Landed: { commits: 2, kept: "Local" } } },
      },
    ]);
    const html = view(after);
    expect(html).toContain("Finishing on its issue");
    expect(html).toContain("Finished: 2 commits landed, kept locally");
  });
});
