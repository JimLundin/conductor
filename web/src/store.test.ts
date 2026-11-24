import { Effect, Result } from "effect";
import { expect, test } from "vitest";
import { fakeHost } from "./fake-pipe.ts";
import { scenario, scoped, serve } from "./link/replay.ts";
import { Route } from "./route.ts";
import { makeStore, sessionOf, sessionsOf } from "./store.ts";
import { keysOf, memoryStorage, page, when } from "./test-page.ts";

// crates/link/src/ticket.rs's worked example, and the host it names.
const exampleTicket =
	"AFhmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmAQEBAQEBAQEBAQEBAQEBAWh0dHBzOi8vci5leGFtcGxlLw";
const exampleHost = `58${"66".repeat(31)}`;
const exampleRecord = `conductor.v2.host.${exampleHost}`;

// The Dioxus app's test ticket, a host on the n0 relay.
const otherTicket =
	"AOr1251Bb2p0aTNfacOjGWA_ymOblEoyognrqD1bXfA_j1NbKmHUzEVqXP2oBgk2pGh0dHBzOi8vZXVjMS0xLnJlbGF5Lm4wLmlyb2gubGluay4v";

const turn = scenario("a_claude_turn_streams_asks_and_finishes");

test("opening a join link pairs its host, keeps its record and drops the ticket from the page", async () => {
	const storage = memoryStorage();
	const { boundary, at } = page(`#join=${exampleTicket}`, storage);
	const view = await scoped(
		makeStore(boundary).pipe(Effect.map((store) => store.view())),
	);
	expect([...view.hosts.keys()]).toEqual([exampleHost]);
	expect(view.route._tag).toBe("Sessions");
	expect(at()).toBe("#/");
	expect(keysOf(storage)).toEqual([exampleRecord]);
});

test("a pasted ticket or join link pairs a second host beside the first", async () => {
	const storage = memoryStorage();
	const { first, second, hosts } = await scoped(
		Effect.gen(function* pairing() {
			const store = yield* makeStore(page("", storage).boundary);
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

test("a paired host is restored from its record on the next launch, with the name it gave", async () => {
	const storage = memoryStorage();
	const welcome = await Effect.runPromise(turn.welcome(1));
	await scoped(
		Effect.gen(function* first() {
			const host = yield* Effect.promise(fakeHost);
			yield* serve(host, {
				reply: { Welcome: welcome },
				feed: [],
				results: [],
			});
			const store = yield* makeStore(
				page(`#join=${exampleTicket}`, storage, host.pipe).boundary,
			);
			yield* when(
				store,
				(shown) => shown.view().hosts.get(exampleHost)?.saved !== null,
			);
		}),
	);
	const restored = await scoped(
		makeStore(page("", storage).boundary).pipe(
			Effect.map((store) => store.view().hosts.get(exampleHost)),
		),
	);
	expect(restored?.saved).toBe(welcome.host);
	expect(restored?.status.state).toBe("Disconnected");
});

test("pairing a host again with a new ticket keeps its record and its name", async () => {
	const storage = memoryStorage();
	const welcome = await Effect.runPromise(turn.welcome(1));
	// The same host with another token: bytes 33 to 35 are the token's first three.
	const renewed = `${exampleTicket.slice(0, 44)}AgIC${exampleTicket.slice(48)}`;
	const view = await scoped(
		Effect.gen(function* again() {
			const host = yield* Effect.promise(fakeHost);
			yield* serve(host, {
				reply: { Welcome: welcome },
				feed: [],
				results: [],
			});
			const store = yield* makeStore(page("", storage, host.pipe).boundary);
			store.pair(exampleTicket);
			yield* when(
				store,
				(shown) => shown.view().hosts.get(exampleHost)?.saved !== null,
			);
			store.pair(renewed);
			return store.view();
		}),
	);
	expect([...view.hosts.keys()]).toEqual([exampleHost]);
	expect(view.hosts.get(exampleHost)?.ticket.token.slice(0, 4)).toEqual([
		2, 2, 2, 1,
	]);
	expect(view.hosts.get(exampleHost)?.saved).toBe(welcome.host);
	expect(keysOf(storage)).toEqual([exampleRecord]);
	expect(storage.getItem(exampleRecord)).toContain(welcome.host);
});

test("a record the Dioxus app kept is not read", async () => {
	const storage = memoryStorage([
		[
			`conductor.host.${exampleHost}`,
			JSON.stringify({
				[exampleHost]: { ticket: exampleTicket, name: "desk" },
			}),
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

test("a session link in lower case opens the session the host wrote in upper case", async () => {
	const session = "01JQ8ZK3VYN7M2WXR4TBHC5DE1";
	const view = await scoped(
		Effect.gen(function* opening() {
			const host = yield* Effect.promise(fakeHost);
			yield* serve(host, {
				reply: { Welcome: yield* turn.welcome(1) },
				feed: turn.feed(1),
				results: [],
			});
			const shown = page(`#join=${exampleTicket}`, memoryStorage(), host.pipe);
			const store = yield* makeStore(shown.boundary);
			yield* when(
				store,
				(opened) => sessionsOf(opened.view(), exampleHost).length > 0,
			);
			shown.boundary.go(
				`#/host/${exampleHost}/session/01jq8zk3vyn7m2wxr4tbhc5de1`,
			);
			store.follow();
			return store.view();
		}),
	);
	expect(view.route).toEqual(Route.Session({ host: exampleHost, session }));
	expect(
		view.route._tag === "Session" &&
			sessionOf(view, exampleHost, view.route.session),
	).toBeTruthy();
});
