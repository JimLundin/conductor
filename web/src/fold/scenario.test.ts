import { expect, test } from "vitest";
import type { Event, Feed, Welcome } from "../proto/index.ts";
import { type RecordedFeed, recordedFeed } from "../recorded-feeds.ts";
import {
	applyFeed,
	applyWelcome,
	emptyHost,
	type Host,
	resetHistory,
} from "./host.ts";
import { sessionOf } from "./examples.ts";
import { controls, openRequests } from "./offers.ts";
import { type Session, SessionStatus } from "./session.ts";
import { Approval, Item, RequestState, ToolState } from "./transcript.ts";

const scenario = "a_claude_turn_streams_asks_and_finishes";
const session = "00000000000000000000000001";
const call = "toolu_bdrk_011tLLSAecKzNjYWMVcNwMfD";
const reply =
	"I created `a.txt` in `/tmp/s1-scratch` with the content `hello from s1`.";
const written = {
	FileChange: {
		files: [{ change: "Write", path: "/tmp/s1-scratch/a.txt" }],
	},
} as const;
const approved = {
	Decided: { by: "test-client", decision: "Approve" },
} as const;

type Frame = Welcome | Feed;

// What one connection of the scenario's test client received: its welcome, then its feed.
const received = (feed: RecordedFeed, connection: number): readonly Frame[] =>
	feed
		.filter((recorded) => recorded.connection === connection)
		.flatMap((recorded): readonly Frame[] => {
			if (recorded.stream === "hello" && "Welcome" in recorded.frame) {
				return [recorded.frame.Welcome];
			}
			return recorded.stream === "feed" ? [recorded.frame] : [];
		});

const fold = (host: Host, frame: Frame): Host =>
	"host" in frame ? applyWelcome(host, frame) : applyFeed(host, frame);

const folded = (frames: readonly Frame[]): Host =>
	frames.reduce(fold, emptyHost());

const stored = (frame: Frame): Event | undefined =>
	"Event" in frame && !isDelta(frame.Event) ? frame.Event : undefined;

const isDelta = (event: Event): boolean =>
	typeof event.kind === "object" &&
	("AssistantDelta" in event.kind || "ReasoningDelta" in event.kind);

const at = (frames: readonly Frame[], seq: number): number =>
	frames.findIndex((frame) => stored(frame)?.seq === seq);

// The frames through the stored event at `seq`.
const through = (frames: readonly Frame[], seq: number): readonly Frame[] =>
	frames.slice(0, at(frames, seq) + 1);

// The frames before the stored event at `seq`.
const before = (frames: readonly Frame[], seq: number): readonly Frame[] =>
	frames.slice(0, at(frames, seq));

const theSession = (host: Host): Session => sessionOf(host, session);

test("a welcome names the host and brings its catalog and instances", () => {
	const host = folded(received(recordedFeed(scenario), 0).slice(0, 1));
	expect(host.name).toBe("test-host");
	expect(Object.keys(host.catalog ?? {})).toStrictEqual(["Claude", "Codex"]);
	expect(host.instances.map(([instance]) => instance)).toStrictEqual([
		{ account: "Own", provider: "Claude" },
		{ account: "Own", provider: "Codex" },
	]);
	expect(host.sessions.size).toBe(0);
});

test("a finished turn's session shows its message, its tool card with the approval, and its reply", () => {
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

test("an approval the agent waits on is the session's open request", () => {
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

test("the reply streams in its deltas until its final text replaces them", () => {
	const streaming = folded(before(received(recordedFeed(scenario), 0), 11));
	expect(theSession(streaming).items.at(-1)).toStrictEqual(
		Item.Reply({ text: reply, state: "Streaming" }),
	);
});

test("the composer opens and Interrupt is offered exactly while the scenario's session allows them", () => {
	const frames = received(recordedFeed(scenario), 0);
	const offered = frames.flatMap((frame, index) => {
		const event = stored(frame);
		if (event === undefined) {
			return [];
		}
		return [
			[event.seq, controls(theSession(folded(frames.slice(0, index + 1))))],
		];
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
	applyFeed(host, { Event: { seq, session, kind } });

test("Continue is offered once the scenario's session closes, and gone once it is ready again", () => {
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

test("Finish is offered for a session on an issue when it is idle or closed, and not while it finishes", () => {
	const frames = received(recordedFeed(scenario), 0);
	const onIssue = then(folded(through(frames, 4)), 5, {
		OnIssue: { issue: "0twr04e5" },
	});
	expect(controls(theSession(onIssue)).finish).toBe(true);
	const working = applyFeed(onIssue, {
		Event: {
			seq: 6,
			session,
			kind: { UserMessage: { by: "test-client", text: "create a.txt" } },
		},
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

test("a request resolved on another device shows as resolved after catch-up", () => {
	const feed = recordedFeed(scenario);
	const elsewhere = folded(received(feed, 1));
	expect(openRequests(theSession(elsewhere))).toHaveLength(1);
	// Connection 0 approves; connection 1 reconnects, and its subscription after seq 8 replays
	// the stored events that followed, never the live deltas.
	const replay = received(feed, 0).filter(
		(frame) => (stored(frame)?.seq ?? 0) > 8,
	);
	const caughtUp = replay.reduce(
		fold,
		applyWelcome(elsewhere, welcomeOf(feed)),
	);
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

const welcomeOf = (feed: RecordedFeed): Welcome => {
	const [welcome] = received(feed, 1);
	if (welcome === undefined || !("host" in welcome)) {
		throw new Error("the connection opened without a welcome");
	}
	return welcome;
};

test("a reconnect's welcome keeps the sessions, and a reset clears them before the replay", () => {
	const feed = recordedFeed(scenario);
	const host = applyWelcome(folded(received(feed, 0)), welcomeOf(feed));
	expect(host.sessions.size).toBe(1);
	expect(resetHistory(host).sessions.size).toBe(0);
	expect(resetHistory(host).name).toBe("test-host");
});
