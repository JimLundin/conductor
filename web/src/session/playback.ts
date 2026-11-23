// A screen's command played against a fake host over the fake pipe: the host the app folded
// before it, the command the host received, its result, and the host after the feed that
// answers it.
import { Effect, Ref } from "effect";
import { fakeHost } from "../fake-pipe.ts";
import {
	applyFeed,
	applyWelcome,
	emptyHost,
	type Host,
	resetHistory,
} from "../fold/host.ts";
import type { SendError } from "../link/command.ts";
import type { Update } from "../link/host.ts";
import {
	client,
	scenario,
	scoped,
	serve,
	throughFeed,
	ticket,
} from "../link/replay.ts";
import { link } from "../link.ts";
import type {
	Command,
	CommandBody,
	CommandResult,
	EventKind,
	Feed,
	SessionId,
	Welcome,
} from "../proto/index.ts";

const recorded = scenario("a_claude_turn_streams_asks_and_finishes");

const folded = (host: Host, update: Update): Host => {
	if (update._tag === "Reset") {
		return resetHistory(host);
	}
	if (update._tag === "Welcome") {
		return applyWelcome(host, update.welcome);
	}
	if (update._tag === "Feed") {
		return applyFeed(host, update.frame);
	}
	return host;
};

const hostOf = (updates: readonly Update[]): Host =>
	updates.reduce(folded, emptyHost());

interface Play {
	readonly welcome?: Welcome;
	// What the host replays on subscribing; never empty, so the app knows it caught up.
	readonly feed: readonly Feed[];
	readonly result: CommandResult;
	// What the host's log says after the command, on the same subscription.
	readonly answer: readonly Feed[];
}

interface Played {
	readonly before: Host;
	readonly commands: readonly CommandBody[];
	readonly result: CommandResult | SendError;
	readonly after: Host;
}

// Plays `play`, sending the command `act` picks from what the app folded before it.
const played = (
	play: Play,
	act: (before: Host) => CommandBody,
): Promise<Played> =>
	scoped(
		Effect.gen(function* playing() {
			const fake = yield* Effect.promise(fakeHost);
			const welcome = play.welcome ?? (yield* recorded.welcome(0));
			const served = yield* serve(fake, {
				reply: { Welcome: welcome },
				feed: play.feed,
				results: [play.result],
				answers: [play.answer],
			});
			const linked = yield* link(fake.pipe, ticket, client);
			const caughtUp = yield* throughFeed(linked, play.feed.length);
			const before = hostOf(caughtUp);
			const sent = yield* linked.send(act(before)).pipe(
				Effect.match({
					onFailure: (error): CommandResult | SendError => error,
					onSuccess: (result): CommandResult | SendError => result,
				}),
			);
			const answered =
				play.answer.length === 0
					? []
					: yield* throughFeed(linked, play.answer.length);
			const commands = yield* Ref.get(served.commands);
			return {
				before,
				commands: commands.map((command: Command) => command.body),
				result: sent,
				after: hostOf([...caughtUp, ...answered]),
			};
		}),
	);

// The recorded scenario's first connection: its frames for the events from `from` through
// `through`, with its frames that are not events when it starts from 0.
const recordedSpan = (from: number, through: number): readonly Feed[] =>
	recorded.feed(0).filter((frame) => {
		if (!("Event" in frame)) {
			return from === 0;
		}
		return frame.Event.seq >= from && frame.Event.seq <= through;
	});

const session: SessionId = "00000000000000000000000001";

// Frames for `kinds` on `session`, numbered from `first` as the host stores them.
const framed = (first: number, kinds: readonly EventKind[]): readonly Feed[] =>
	kinds.map((kind, at) => ({ Event: { seq: first + at, session, kind } }));

export { framed, played, recordedSpan, session };
