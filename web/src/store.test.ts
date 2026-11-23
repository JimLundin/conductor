import { Effect, Result } from "effect";
import { expect, test } from "vitest";
import { fakeHost } from "./fake-pipe.ts";
import { dialing, scenario, scoped, serve } from "./link/replay.ts";
import { memoryStorage } from "./memory-storage.ts";
import { type Boundary, makeStore, type Store, sessionsOf } from "./store.ts";

// crates/proto/src/ticket.rs's worked example, and the host it names.
const exampleTicket =
	"AFhmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmAQEBAQEBAQEBAQEBAQEBAWh0dHBzOi8vci5leGFtcGxlLw";
const exampleHost = `58${"66".repeat(31)}`;

// The Dioxus app's test ticket, a host on the n0 relay.
const otherTicket =
	"AOr1251Bb2p0aTNfacOjGWA_ymOblEoyognrqD1bXfA_j1NbKmHUzEVqXP2oBgk2pGh0dHBzOi8vZXVjMS0xLnJlbGF5Lm4wLmlyb2gubGluay4v";

interface Page {
	readonly boundary: Boundary;
	readonly at: () => string;
}

const page = (
	start: string,
	storage: Storage,
	pipe = dialing([]),
): Page => {
	let fragment = start;
	const move = (next: string): void => {
		fragment = next;
	};
	return {
		boundary: {
			pipe,
			client: "test-client",
			storage,
			fragment: (): string => fragment,
			go: move,
			replace: move,
		},
		at: (): string => fragment,
	};
};

const keysOf = (storage: Storage): readonly string[] =>
	Array.from({ length: storage.length }, (_, index) => storage.key(index) ?? "");

// Resolves once `holds` is true of the store's view.
const when = (store: Store, holds: (store: Store) => boolean): Effect.Effect<void> =>
	Effect.callback<void>((resume) => {
		if (holds(store)) {
			resume(Effect.void);
			return;
		}
		const stop = store.subscribe(() => {
			if (holds(store)) {
				stop();
				resume(Effect.void);
			}
		});
	});

test("opening a join link pairs its host, keeps its record and drops the ticket from the page", async () => {
	const storage = memoryStorage();
	const { boundary, at } = page(`#join=${exampleTicket}`, storage);
	const view = await scoped(
		makeStore(boundary).pipe(Effect.map((store) => store.view())),
	);
	expect([...view.hosts.keys()]).toEqual([exampleHost]);
	expect(view.route._tag).toBe("Sessions");
	expect(at()).toBe("#/");
	expect(keysOf(storage)).toEqual([`conductor.v2.host.${exampleHost}`]);
});

test("a pasted ticket or join link pairs a second host beside the first", async () => {
	const storage = memoryStorage();
	const { boundary } = page("", storage);
	const { first, second, hosts } = await scoped(
		Effect.gen(function* pairing() {
			const store = yield* makeStore(boundary);
			const paired = store.pair(`  ${exampleTicket}\n`);
			const again = store.pair(
				`https://jimlundin.github.io/conductor/#join=${otherTicket}`,
			);
			return {
				first: paired,
				second: again,
				hosts: [...store.view().hosts.keys()],
			};
		}),
	);
	expect(first).toEqual(Result.succeed(exampleHost));
	expect(Result.isSuccess(second)).toBe(true);
	expect(hosts).toHaveLength(2);
	expect(keysOf(storage)).toHaveLength(2);
});

test("a paired host is restored from its record on the next launch", async () => {
	const storage = memoryStorage();
	await scoped(
		makeStore(page(`#join=${exampleTicket}`, storage).boundary),
	);
	const hosts = await scoped(
		makeStore(page("", storage).boundary).pipe(
			Effect.map((store) => [...store.view().hosts.keys()]),
		),
	);
	expect(hosts).toEqual([exampleHost]);
});

test("a record the Dioxus app kept is not read", async () => {
	const storage = memoryStorage([
		[
			`conductor.host.${exampleHost}`,
			JSON.stringify({ [exampleHost]: { ticket: exampleTicket, name: "desk" } }),
		],
		[`conductor.name.${exampleHost}`, "desk"],
	]);
	const view = await scoped(
		makeStore(page("", storage).boundary).pipe(
			Effect.map((store) => store.view()),
		),
	);
	expect(view.hosts.size).toBe(0);
	expect(view.route._tag).toBe("Pairing");
});

test("a malformed join link pairs nothing and says why", async () => {
	const storage = memoryStorage();
	const view = await scoped(
		makeStore(page("#join=not base64!", storage).boundary).pipe(
			Effect.map((store) => store.view()),
		),
	);
	expect(view.hosts.size).toBe(0);
	expect(view.notice?._tag).toBe("Encoding");
	expect(keysOf(storage)).toEqual([]);
});

const turn = scenario("a_claude_turn_streams_asks_and_finishes");

test("forgetting a host removes its record and its sessions", async () => {
	const storage = memoryStorage();
	const { before, after } = await scoped(
		Effect.gen(function* forgetting() {
			const host = yield* Effect.promise(fakeHost);
			yield* serve(host, {
				reply: { Welcome: yield* turn.welcome(1) },
				feed: turn.feed(1),
				results: [],
			});
			const store = yield* makeStore(
				page(`#join=${exampleTicket}`, storage, host.pipe).boundary,
			);
			yield* when(
				store,
				(shown) => sessionsOf(shown.view(), exampleHost).length > 0,
			);
			const sessions = sessionsOf(store.view(), exampleHost);
			store.forget(exampleHost);
			return { before: sessions, after: store.view() };
		}),
	);
	expect(before).not.toHaveLength(0);
	expect(after.hosts.size).toBe(0);
	expect(sessionsOf(after, exampleHost)).toEqual([]);
	expect(keysOf(storage)).toEqual([]);
});
