import { describe, expect, it } from "@effect/vitest";
import { Update } from "#ui/link/host.ts";
import type { Event } from "@conductor/core/schemas/index";
import { type RecordedFeed, recordedFeed } from "#ui/test/recorded-feeds.ts";
import { isDelta } from "#ui/link/feed.ts";
import { applyUpdate, type Host } from "@conductor/core/fold/host";
import { foldUpdates as folded, received, sessionOf } from "#ui/test/examples.ts";
import { controls, openRequests } from "@conductor/core/fold/offers";
import { type Session, SessionStatus } from "@conductor/core/fold/session";
import { Approval, Item, RequestState, ToolState } from "@conductor/core/fold/transcript";

describe("scenario", () => {
  const scenario = "a_claude_turn_streams_asks_and_finishes";
  const session = "00000000000000000000000001";
  const call = "toolu_bdrk_011tLLSAecKzNjYWMVcNwMfD";
  const reply = "I created `a.txt` in `/tmp/s1-scratch` with the content `hello from s1`.";
  const written = {
    FileChange: {
      files: [{ change: "Write", path: "/tmp/s1-scratch/a.txt" }],
    },
  } as const;
  const approved = {
    Decided: { by: "test-client", decision: "Approve" },
  } as const;

  const stored = (update: Update): Event | undefined =>
    update._tag === "Feed" && "Event" in update.frame && !isDelta(update.frame.Event)
      ? update.frame.Event
      : undefined;

  const at = (updates: readonly Update[], seq: number): number =>
    updates.findIndex((update) => stored(update)?.seq === seq);

  // The updates through the stored event at `seq`.
  const through = (updates: readonly Update[], seq: number): readonly Update[] =>
    updates.slice(0, at(updates, seq) + 1);

  // The updates before the stored event at `seq`.
  const before = (updates: readonly Update[], seq: number): readonly Update[] =>
    updates.slice(0, at(updates, seq));

  const theSession = (host: Host): Session => sessionOf(host, session);

  it("a welcome names the host and brings its catalog and instances", () => {
    expect.hasAssertions();
    const host = folded(received(recordedFeed(scenario), 0).slice(0, 1));
    expect(host.name).toBe("test-host");
    expect(Object.keys(host.catalog ?? {})).toStrictEqual(["claude", "codex"]);
    expect(host.instances.map(([instance]) => instance)).toStrictEqual([
      { account: "Own", provider: "claude" },
      { account: "Own", provider: "codex" },
    ]);
    expect(host.sessions.size).toBe(0);
  });

  it("a finished turn's session shows its message, its tool card with the approval, and its reply", () => {
    expect.hasAssertions();
    const host = folded(received(recordedFeed(scenario), 0));
    expect(theSession(host).items).toStrictEqual([
      Item.User({ text: "create a.txt", by: "test-client" }),
      Item.Tool({
        call,
        tool: written,
        state: ToolState.Finished({ outcome: "Succeeded" }),
        approval: Approval.Asked({
          request: 1,
          reason: null,
          forSession: null,
          state: RequestState.Resolved({ resolution: approved }),
        }),
      }),
      Item.Reply({ text: reply, state: "Done" }),
      Item.TurnCompleted({ outcome: "Completed" }),
    ]);
    expect(theSession(host).status).toStrictEqual(SessionStatus.Idle());
    expect(openRequests(theSession(host))).toStrictEqual([]);
  });

  it("an approval the agent waits on is the session's open request", () => {
    expect.hasAssertions();
    const host = folded(through(received(recordedFeed(scenario), 0), 8));
    expect(openRequests(theSession(host))).toStrictEqual([
      Item.Tool({
        call,
        tool: written,
        state: ToolState.Running(),
        approval: Approval.Asked({
          request: 1,
          reason: null,
          forSession: null,
          state: RequestState.Open(),
        }),
      }),
    ]);
  });

  it("the reply streams in its deltas until its final text replaces them", () => {
    expect.hasAssertions();
    const streaming = folded(before(received(recordedFeed(scenario), 0), 11));
    expect(theSession(streaming).items.at(-1)).toStrictEqual(
      Item.Reply({ text: reply, state: "Streaming" }),
    );
  });

  it("the composer opens and Interrupt is offered exactly while the scenario's session allows them", () => {
    expect.hasAssertions();
    const frames = received(recordedFeed(scenario), 0);
    const offered = frames.flatMap((frame, index) => {
      const event = stored(frame);
      if (event === undefined) {
        return [];
      }
      return [[event.seq, controls(theSession(folded(frames.slice(0, index + 1))))]];
    });
    const busy = {
      composer: "Busy",
      interrupt: false,
      continue: false,
      finish: false,
    };
    const open = {
      composer: "Open",
      interrupt: false,
      continue: false,
      finish: false,
    };
    const running = {
      composer: "Busy",
      interrupt: true,
      continue: false,
      finish: false,
    };
    expect(offered).toStrictEqual([
      [1, busy],
      [2, busy],
      [3, busy],
      [4, open],
      [5, running],
      [6, running],
      [7, running],
      [8, running],
      [9, running],
      [10, running],
      [11, running],
      [12, running],
      [13, open],
    ]);
  });

  // The feed's sessions never close and aren't on an issue, so these cases extend its frames with
  // the events the host logs when the agent exits and when a session finishes on its issue.
  const then = (host: Host, seq: number, kind: Event["kind"]): Host =>
    applyUpdate(host, Update.Feed({ frame: { Event: { seq, session, kind } } }));

  it("Continue is offered once the scenario's session closes, and gone once it is ready again", () => {
    expect.hasAssertions();
    const finished = folded(received(recordedFeed(scenario), 0));
    const exited = then(finished, 14, {
      SessionClosed: { reason: { Exited: { status: "1" } } },
    });
    expect(controls(theSession(exited))).toStrictEqual({
      composer: "Closed",
      interrupt: false,
      continue: true,
      finish: false,
    });
    const continued = then(exited, 15, "SessionReady");
    expect(controls(theSession(continued)).continue).toBe(false);
    expect(theSession(continued).items.at(-1)).toStrictEqual(
      Item.Continued({ reason: { Exited: { status: "1" } } }),
    );
  });

  it("Finish is offered for a session on an issue when it is idle or closed, and not while it finishes", () => {
    expect.hasAssertions();
    const updates = received(recordedFeed(scenario), 0);
    const onIssue = then(folded(through(updates, 4)), 5, {
      OnIssue: { issue: "0twr04e5" },
    });
    expect(controls(theSession(onIssue)).finish).toBe(true);
    const working = then(onIssue, 6, {
      UserMessage: { by: "test-client", text: "create a.txt" },
    });
    expect(controls(theSession(working)).finish).toBe(false);
    const idle = then(working, 7, { TurnCompleted: { outcome: "Completed" } });
    expect(controls(theSession(idle)).finish).toBe(true);
    const finishing = then(idle, 8, "FinishStarted");
    expect(controls(theSession(finishing)).finish).toBe(false);
    const retrying = then(finishing, 9, "FinishInterrupted");
    expect(controls(theSession(retrying)).finish).toBe(true);
    const done = then(then(retrying, 10, "FinishStarted"), 11, {
      SessionFinished: { finished: { Failed: { reason: "merge conflict" } } },
    });
    expect(controls(theSession(done)).finish).toBe(false);
    expect(theSession(done).items.slice(-3)).toStrictEqual([
      Item.Finishing(),
      Item.Finishing(),
      Item.Finished({ finished: { Failed: { reason: "merge conflict" } } }),
    ]);
  });

  const welcomeOf = (feed: RecordedFeed): Update => {
    const [welcome] = received(feed, 1);
    if (welcome?._tag !== "Welcome") {
      throw new Error("the connection opened without a welcome");
    }
    return welcome;
  };

  it("a request resolved on another device shows as resolved after catch-up", () => {
    expect.hasAssertions();
    const feed = recordedFeed(scenario);
    const elsewhere = received(feed, 1);
    expect(openRequests(theSession(folded(elsewhere)))).toHaveLength(1);
    // Connection 0 approves; connection 1 reconnects, and its subscription after seq 8 replays
    // the stored events that followed, never the live deltas.
    const replay = received(feed, 0).filter((update) => (stored(update)?.seq ?? 0) > 8);
    const caughtUp = folded([...elsewhere, welcomeOf(feed), ...replay]);
    expect(openRequests(theSession(caughtUp))).toStrictEqual([]);
    expect(theSession(caughtUp).items[1]).toStrictEqual(
      Item.Tool({
        call,
        tool: written,
        state: ToolState.Finished({ outcome: "Succeeded" }),
        approval: Approval.Asked({
          request: 1,
          reason: null,
          forSession: null,
          state: RequestState.Resolved({ resolution: approved }),
        }),
      }),
    );
  });

  it("a reconnect's welcome keeps the sessions, and a reset clears them before the replay", () => {
    expect.hasAssertions();
    const feed = recordedFeed(scenario);
    const host = folded([...received(feed, 0), welcomeOf(feed)]);
    expect(host.sessions.size).toBe(1);
    const reset = applyUpdate(host, Update.Reset({ epoch: "reborn" }));
    expect(reset.sessions.size).toBe(0);
    expect(reset.name).toBe("test-host");
  });

  it("a link status leaves the fold as it was", () => {
    expect.hasAssertions();
    const host = folded(received(recordedFeed(scenario), 0));
    const down = Update.Status({ state: "Disconnected", error: "closed" });
    expect(applyUpdate(host, down)).toStrictEqual(host);
  });
});
