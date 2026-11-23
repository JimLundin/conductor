import { Effect, Ref } from "effect";
import { expect, test } from "vitest";
import { fakeHost } from "../fake-pipe.ts";
import { link } from "../link.ts";
import type { Connection } from "../pipe.ts";
import type { Command } from "../proto/index.ts";
import {
	client,
	dialing,
	inState,
	scenario,
	scoped,
	serve,
	ticket,
	until,
} from "./replay.ts";

const turn = scenario("a_claude_turn_streams_asks_and_finishes");

const interrupt: Command = {
	Session: { Interrupt: { session: "00000000000000000000000001" } },
} as const;

const ulid = /^[0-7][0-9a-hjkmnp-tv-z]{25}$/u;

test("each command gets its own stream and returns its own result to its caller", async () => {
	const results = turn.results(0);
	const { answers, commands } = await scoped(
		Effect.gen(function* commanding() {
			const host = yield* Effect.promise(fakeHost);
			const reply = { Welcome: yield* turn.welcome(0) };
			const served = yield* serve(host, { reply, feed: turn.feed(0), results });
			const linked = yield* link(host.pipe, ticket, client);
			yield* until(linked, inState("Live"));
			const answered = yield* Effect.forEach(results, () =>
				linked.send(interrupt),
			);
			return { answers: answered, commands: yield* Ref.get(served.commands) };
		}),
	);
	expect(answers).toEqual(results);
	expect(commands.map((command) => command.body)).toEqual(
		results.map(() => interrupt),
	);
	expect(new Set(commands.map((command) => command.id)).size).toBe(
		results.length,
	);
	expect(commands.filter((command) => ulid.test(command.id))).toEqual(commands);
});

test("a command while the link is down fails as not connected", async () => {
	const failed = await scoped(
		Effect.gen(function* down() {
			const unreachable = (): Promise<Connection> =>
				Promise.reject(new Error("no route"));
			const linked = yield* link(dialing([unreachable]), ticket, client);
			yield* until(linked, inState("Disconnected"));
			return yield* Effect.flip(linked.send(interrupt));
		}),
	);
	expect(failed._tag).toBe("NotConnected");
});

test("a command whose stream ends without a result fails as such", async () => {
	const failed = await scoped(
		Effect.gen(function* unanswered() {
			const host = yield* Effect.promise(fakeHost);
			yield* serve(host, {
				reply: { Welcome: yield* turn.welcome(0) },
				feed: turn.feed(0),
				results: [],
			});
			const linked = yield* link(host.pipe, ticket, client);
			yield* until(linked, inState("Live"));
			return yield* Effect.flip(linked.send(interrupt));
		}),
	);
	expect(failed._tag).toBe("NoResult");
});
