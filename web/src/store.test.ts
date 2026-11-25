import { Effect, Result } from "effect";
import { describe, expect, it } from "@effect/vitest";
import { keysOf, memoryStorage, page, when } from "#ui/test/page.ts";
import { scenario, serve } from "#ui/test/replay.ts";
import { Route } from "@conductor/core/route";
import type { Store } from "#ui/store.ts";
import { TicketError } from "@conductor/core/ticket";
import example from "#ticket-example" with { type: "json" };
import { fakeHost } from "#ui/test/fake-pipe.ts";
import { makeStore } from "#ui/store.ts";
import { sessionsOf } from "@conductor/core/store";

// The ticket link writes as its worked example, and the host it names.
const exampleTicket = example.text;
const exampleHost = `58${"66".repeat(31)}`;
const exampleRecord = `conductor.v2.host.${exampleHost}`;

// The Dioxus app's test ticket, a host on the n0 relay.
const otherTicket =
  "AOr1251Bb2p0aTNfacOjGWA_ymOblEoyognrqD1bXfA_j1NbKmHUzEVqXP2oBgk2pGh0dHBzOi8vZXVjMS0xLnJlbGF5Lm4wLmlyb2gubGluay4v";

const turn = (): ReturnType<typeof scenario> => scenario("a_claude_turn_streams_asks_and_finishes");

describe("join links", () => {
  it.effect(
    "opening a join link pairs its host, keeps its record and drops the ticket from the page",
    () =>
      Effect.gen(function* verify1() {
        const storage = memoryStorage();
        const { boundary, at } = page(`#join=${exampleTicket}`, storage);
        const view = yield* Effect.scoped(
          makeStore(boundary).pipe(Effect.map((store: Pick<Store, "view">) => store.view())),
        );
        expect([...view.hosts.keys()]).toStrictEqual([exampleHost]);
        expect(view.route).toStrictEqual(Route.Sessions());
        expect(at()).toBe("#/");
        expect(keysOf(storage)).toStrictEqual([exampleRecord]);
      }),
  );
});

describe("multiple hosts", () => {
  it.effect("a pasted ticket or join link pairs a second host beside the first", () =>
    Effect.gen(function* verify2() {
      const storage = memoryStorage();
      const { first, second, hosts } = yield* Effect.scoped(
        Effect.gen(function* pairing() {
          const store = yield* makeStore(page("", storage).boundary);
          const paired = store.pair(`  ${exampleTicket}\n`);
          const again = store.pair(`https://jimlundin.github.io/conductor/#join=${otherTicket}`);
          return {
            first: paired,
            hosts: [...store.view().hosts.keys()],
            second: again,
          };
        }),
      );
      expect(first).toStrictEqual(Result.succeed(exampleHost));
      expect(Result.isSuccess(second)).toBe(true);
      expect(hosts).toHaveLength(2);
      expect(keysOf(storage)).toHaveLength(2);
    }),
  );
});

describe("restored pairing", () => {
  it.effect(
    "a paired host is restored from its record on the next launch, with the name it gave",
    () =>
      Effect.gen(function* verify3() {
        const storage = memoryStorage();
        const welcome = yield* turn().welcome(1);
        yield* Effect.scoped(
          Effect.gen(function* first() {
            const host = yield* Effect.tryPromise(fakeHost);
            yield* serve(host, {
              feed: [],
              reply: { Welcome: welcome },
              results: [],
            });
            const store = yield* makeStore(
              page(`#join=${exampleTicket}`, storage, host.pipe).boundary,
            );
            yield* when(store, (shown) => shown.view().hosts.get(exampleHost)?.saved !== undefined);
          }),
        );
        const restored = yield* Effect.scoped(
          makeStore(page("", storage).boundary).pipe(
            Effect.map((store: Pick<Store, "view">) => store.view().hosts.get(exampleHost)),
          ),
        );
        expect(restored?.saved).toBe(welcome.host);
        expect(restored?.status.state).toBe("Disconnected");
      }),
  );
});

describe("replaced tickets", () => {
  it.effect("pairing a host again with a new ticket keeps its record and its name", () =>
    Effect.gen(function* verify4() {
      const storage = memoryStorage();
      const welcome = yield* turn().welcome(1);
      // The same host with another token: bytes 33 to 35 are the token's first three.
      const renewed = `${exampleTicket.slice(0, 44)}AgIC${exampleTicket.slice(48)}`;
      const view = yield* Effect.scoped(
        Effect.gen(function* again() {
          const host = yield* Effect.tryPromise(fakeHost);
          yield* serve(host, {
            feed: [],
            reply: { Welcome: welcome },
            results: [],
          });
          const store = yield* makeStore(page("", storage, host.pipe).boundary);
          store.pair(exampleTicket);
          yield* when(store, (shown) => shown.view().hosts.get(exampleHost)?.saved !== undefined);
          store.pair(renewed);
          return store.view();
        }),
      );
      expect([...view.hosts.keys()]).toStrictEqual([exampleHost]);
      expect(view.hosts.get(exampleHost)?.ticket.token.slice(0, 4)).toStrictEqual([2, 2, 2, 1]);
      expect(view.hosts.get(exampleHost)?.saved).toBe(welcome.host);
      expect(keysOf(storage)).toStrictEqual([exampleRecord]);
      expect(storage.getItem(exampleRecord)).toContain(welcome.host);
    }),
  );
});

describe("obsolete pairing", () => {
  it.effect("a record the Dioxus app kept is not read", () =>
    Effect.gen(function* verify5() {
      const storage = memoryStorage([
        [
          `conductor.host.${exampleHost}`,
          JSON.stringify({
            [exampleHost]: { name: "desk", ticket: exampleTicket },
          }),
        ],
        [`conductor.name.${exampleHost}`, "desk"],
      ]);
      const view = yield* Effect.scoped(
        makeStore(page("", storage).boundary).pipe(
          Effect.map((store: Pick<Store, "view">) => store.view()),
        ),
      );
      expect(view.hosts.size).toBe(0);
      expect(view.route).toStrictEqual(Route.Pairing());
    }),
  );
});

describe("malformed pairing", () => {
  it.effect("a malformed join link pairs nothing and says why", () =>
    Effect.gen(function* verify6() {
      const storage = memoryStorage();
      const view = yield* Effect.scoped(
        makeStore(page("#join=not base64!", storage).boundary).pipe(
          Effect.map((store: Pick<Store, "view">) => store.view()),
        ),
      );
      expect(view.hosts.size).toBe(0);
      expect(view.notice).toStrictEqual(TicketError.Encoding());
      expect(keysOf(storage)).toStrictEqual([]);
    }),
  );
});

describe("forgotten hosts", () => {
  it.effect("forgetting a host removes its record and its sessions", () =>
    Effect.gen(function* verify7() {
      const storage = memoryStorage();
      const { before, after } = yield* Effect.scoped(
        Effect.gen(function* forgetting() {
          const host = yield* Effect.tryPromise(fakeHost);
          yield* serve(host, {
            feed: turn().feed(1),
            reply: { Welcome: yield* turn().welcome(1) },
            results: [],
          });
          const store = yield* makeStore(
            page(`#join=${exampleTicket}`, storage, host.pipe).boundary,
          );
          yield* when(store, (shown) => sessionsOf(shown.view(), exampleHost).length > 0);
          const sessions = sessionsOf(store.view(), exampleHost);
          store.forget(exampleHost);
          return { after: store.view(), before: sessions };
        }),
      );
      expect(before).not.toHaveLength(0);
      expect(after.hosts.size).toBe(0);
      expect(sessionsOf(after, exampleHost)).toStrictEqual([]);
      expect(keysOf(storage)).toStrictEqual([]);
    }),
  );
});
