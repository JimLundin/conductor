import { Deferred, Effect } from "effect";
import { expect, test } from "vitest";
import { fakeHost } from "../fake-pipe.ts";
import { link } from "../link.ts";
import type { Connection } from "../pipe.ts";
import {
	client,
	dialing,
	inState,
	reach,
	scenario,
	scoped,
	serve,
	states,
	ticket,
	until,
} from "./replay.ts";

const turn = scenario("a_claude_turn_streams_asks_and_finishes");

const stalled = (): Promise<Connection> => Effect.runPromise(Effect.never);

test("a stalled dial goes Live through a fresh dial beside it, and the link stops cleanly", async () => {
	const updates = await scoped(
		Effect.gen(function* hedged() {
			const host = yield* Effect.promise(fakeHost);
			yield* serve(host, {
				reply: { Welcome: yield* turn.welcome(0) },
				feed: turn.feed(0),
				results: [],
			});
			const linked = yield* link(
				dialing([stalled, (): Promise<Connection> => reach(host)]),
				ticket,
				client,
			);
			return yield* until(linked, inState("Live"));
		}),
	);
	expect(states(updates)).toEqual([
		"Connecting",
		"Authenticating",
		"CatchingUp",
		"Live",
	]);
});

test("a stalled dial that connects after the fresh one won is closed", async () => {
	const closed = await scoped(
		Effect.gen(function* hedged() {
			const [host, late] = [
				yield* Effect.promise(fakeHost),
				yield* Effect.promise(fakeHost),
			];
			const connected = yield* Deferred.make<Connection>();
			const slow = (): Promise<Connection> =>
				Effect.runPromise(Deferred.await(connected));
			yield* serve(host, {
				reply: { Welcome: yield* turn.welcome(0) },
				feed: turn.feed(0),
				results: [],
			});
			const linked = yield* link(
				dialing([slow, (): Promise<Connection> => reach(host)]),
				ticket,
				client,
			);
			yield* until(linked, inState("Live"));
			const loser = yield* Effect.promise(() => reach(late));
			yield* Deferred.succeed(connected, loser);
			return yield* Effect.promise(() => loser.closed());
		}),
	);
	expect(closed).toBe("closed");
});
