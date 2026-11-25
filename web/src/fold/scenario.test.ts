import { Approval, Item, RequestState, ToolState } from "@conductor/core/fold/transcript";
import { Array as Arr, Option } from "effect";
import { Update, isDelta } from "@conductor/core/link";
import { controls, openRequests } from "@conductor/core/fold/offers";
import { describe, expect, it } from "@effect/vitest";
import { foldUpdates as folded, received, sessionOf } from "#ui/test/examples.ts";
import type { Event } from "@conductor/core/schemas/index";
import type { Host } from "@conductor/core/fold/host";
import type { RecordedFeed } from "#ui/test/recorded-feeds.ts";
import type { Session } from "@conductor/core/fold/session";
import { SessionStatus } from "@conductor/core/fold/session";
import { applyUpdate } from "@conductor/core/fold/host";
import { recordedFeed } from "#ui/test/recorded-feeds.ts";

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

const approvedState = RequestState.Resolved({ resolution: approved });
const approvedTool = Item.Tool({
  approval: Approval.Asked({
    forSession: undefined,
    reason: undefined,
    request: 1,
    state: approvedState,
  }),
  call,
  state: ToolState.Finished({ outcome: "Succeeded" }),
  tool: written,
});
const openTool = Item.Tool({
  approval: Approval.Asked({
    forSession: undefined,
    reason: undefined,
    request: 1,
    state: RequestState.Open(),
  }),
  call,
  state: ToolState.Running(),
  tool: written,
});

const stored = (update: Update): Event | undefined =>
  Update.$is("Feed")(update) && "Event" in update.frame && !isDelta(update.frame.Event)
    ? update.frame.Event
    : undefined;

const at = (updates: ReadonlyArray<Update>, seq: number): number =>
  updates.findIndex((update) => stored(update)?.seq === seq);

const through = (updates: ReadonlyArray<Update>, seq: number): ReadonlyArray<Update> =>
  updates.slice(0, at(updates, seq) + 1);

const before = (updates: ReadonlyArray<Update>, seq: number): ReadonlyArray<Update> =>
  updates.slice(0, at(updates, seq));

const theSession = (host: Host): Session => sessionOf(host, session);

describe("welcome data", () => {
  it("a welcome names the host and brings its catalog and instances", () => {
    expect.hasAssertions();
    const host = folded(received(recordedFeed(scenario), 0).slice(0, 1));
    expect(host.name).toBe("test-host");
    const catalog = Option.getOrThrow(Option.fromNullishOr(host.catalog));
    expect(Object.keys(catalog)).toStrictEqual(["claude", "codex"]);
    expect(host.instances.map(([instance]) => instance)).toStrictEqual([
      { account: "Own", provider: "claude" },
      { account: "Own", provider: "codex" },
    ]);
    expect(host.sessions.size).toBe(0);
  });
});

describe("finished transcript", () => {
  it("a finished turn's session shows its message, its tool card with the approval, and its reply", () => {
    expect.hasAssertions();
    const host = folded(received(recordedFeed(scenario), 0));
    expect(theSession(host).items).toStrictEqual([
      Item.User({ by: "test-client", text: "create a.txt" }),
      approvedTool,
      Item.Reply({ state: "Done", text: reply }),
      Item.TurnCompleted({ outcome: "Completed" }),
    ]);
    expect(theSession(host).status).toStrictEqual(SessionStatus.Idle());
    expect(openRequests(theSession(host))).toStrictEqual([]);
  });
});

describe("open approvals", () => {
  it("an approval the agent waits on is the session's open request", () => {
    expect.hasAssertions();
    const frames = received(recordedFeed(scenario), 0);
    const host = folded(through(frames, 8));
    expect(openRequests(theSession(host))).toStrictEqual([openTool]);
  });
});

describe("streamed reply", () => {
  it("the reply streams in its deltas until its final text replaces them", () => {
    expect.hasAssertions();
    const frames = received(recordedFeed(scenario), 0);
    const streaming = folded(before(frames, 11));
    expect(theSession(streaming).items.at(-1)).toStrictEqual(
      Item.Reply({ state: "Streaming", text: reply }),
    );
  });
});

const controlsBySequence = (
  frames: ReadonlyArray<Update>,
): ReadonlyArray<readonly [number, ReturnType<typeof controls>]> =>
  frames.flatMap((frame, index) => {
    const event = stored(frame);
    if (event === undefined) {
      return [];
    }
    const host = folded(frames.slice(0, index + 1));
    return [[event.seq, controls(theSession(host))] as const];
  });

describe("turn controls", () => {
  it("the composer opens and Interrupt is offered exactly while the scenario's session allows them", () => {
    expect.hasAssertions();
    const offered = controlsBySequence(received(recordedFeed(scenario), 0));
    const busy = {
      composer: "Busy",
      continue: false,
      finish: false,
      interrupt: false,
    };
    const open = {
      composer: "Open",
      continue: false,
      finish: false,
      interrupt: false,
    };
    const running = {
      composer: "Busy",
      continue: false,
      finish: false,
      interrupt: true,
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
});

const then = (host: Host, seq: number, kind: Event["kind"]): Host =>
  applyUpdate(host, Update.Feed({ frame: { Event: { kind, seq, session } } }));

describe("continuation controls", () => {
  it("continue is offered once the scenario's session closes, and gone once it is ready again", () => {
    expect.hasAssertions();
    const finished = folded(received(recordedFeed(scenario), 0));
    const exited = then(finished, 14, {
      SessionClosed: { reason: { Exited: { status: "1" } } },
    });
    expect(controls(theSession(exited))).toStrictEqual({
      composer: "Closed",
      continue: true,
      finish: false,
      interrupt: false,
    });
    const continued = then(exited, 15, "SessionReady");
    expect(controls(theSession(continued)).continue).toBe(false);
    expect(theSession(continued).items.at(-1)).toStrictEqual(
      Item.Continued({ reason: { Exited: { status: "1" } } }),
    );
  });
});

const finishing = (initial: Host): ReadonlyArray<Host> => {
  const events: ReadonlyArray<Event["kind"]> = [
    { UserMessage: { by: "test-client", text: "create a.txt" } },
    { TurnCompleted: { outcome: "Completed" } },
    "FinishStarted",
    "FinishInterrupted",
    "FinishStarted",
    { SessionFinished: { finished: { Failed: { reason: "merge conflict" } } } },
  ];
  const numbered = events.map((kind, index): Pick<Event, "kind" | "seq"> => ({
    kind,
    seq: index + 6,
  }));
  return Arr.scan(numbered, initial, (host, { kind, seq }) => then(host, seq, kind));
};

describe("finishing controls", () => {
  it("offers finish only while an issue session can finish", () => {
    expect.hasAssertions();
    const updates = received(recordedFeed(scenario), 0);
    const onIssue = then(folded(through(updates, 4)), 5, { OnIssue: { issue: "0twr04e5" } });
    const stages = finishing(onIssue).map((host) => theSession(host));
    const done = Option.getOrThrow(Arr.last(stages));
    expect(stages.map((current) => controls(current).finish)).toStrictEqual([
      true,
      false,
      true,
      false,
      true,
      false,
      false,
    ]);
    expect(done.items.slice(-3)).toStrictEqual([
      Item.Finishing(),
      Item.Finishing(),
      Item.Finished({ finished: { Failed: { reason: "merge conflict" } } }),
    ]);
  });
});

const welcomeOf = (feed: RecordedFeed): Update => {
  const [first] = received(feed, 1);
  const welcome = Option.filter(Option.fromNullishOr(first), Update.$is("Welcome"));
  return Option.getOrThrow(welcome);
};

const replayAfterApproval = (feed: RecordedFeed): ReadonlyArray<Update> =>
  received(feed, 0).filter((update) => (stored(update)?.seq ?? 0) > 8);

describe("catch-up approvals", () => {
  it("a request resolved on another device shows as resolved after catch-up", () => {
    expect.hasAssertions();
    const feed = recordedFeed(scenario);
    const elsewhere = received(feed, 1);
    const waiting = theSession(folded(elsewhere));
    expect(openRequests(waiting)).toHaveLength(1);
    const replay = replayAfterApproval(feed);
    const caughtUp = folded([...elsewhere, welcomeOf(feed), ...replay]);
    expect(openRequests(theSession(caughtUp))).toStrictEqual([]);
    expect(theSession(caughtUp).items[1]).toStrictEqual(approvedTool);
  });
});

describe("replay reset", () => {
  it("a reconnect's welcome keeps the sessions, and a reset clears them before the replay", () => {
    expect.hasAssertions();
    const feed = recordedFeed(scenario);
    const host = folded([...received(feed, 0), welcomeOf(feed)]);
    expect(host.sessions.size).toBe(1);
    const reset = applyUpdate(host, Update.Reset({ epoch: "reborn" }));
    expect(reset.sessions.size).toBe(0);
    expect(reset.name).toBe("test-host");
  });
});

describe("link status", () => {
  it("a link status leaves the fold as it was", () => {
    expect.hasAssertions();
    const host = folded(received(recordedFeed(scenario), 0));
    const down = Update.Status({ error: "closed", state: "Disconnected" });
    expect(applyUpdate(host, down)).toStrictEqual(host);
  });
});
