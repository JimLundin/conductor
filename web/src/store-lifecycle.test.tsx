import { Effect, Result } from "effect";
import { describe, expect, it } from "@effect/vitest";
import { Root } from "#ui/root.tsx";
import { Route } from "@conductor/core/route";
import type { Store } from "#ui/store.ts";
import example from "#ticket-example" with { type: "json" };
import { lifecyclePage } from "#ui/test/lifecycle.ts";
import { makeStore } from "#ui/store.ts";
import { renderToStaticMarkup } from "react-dom/server";
import { restored } from "@conductor/core/saved";
import { when } from "#ui/test/page.ts";

const hostKey = `58${"66".repeat(31)}`;
const recordKey = `conductor.v2.host.${hostKey}`;
const renewed = `${example.text.slice(0, 44)}AgIC${example.text.slice(48)}`;

const settled = (store: Pick<Store, "view" | "subscribe">): Effect.Effect<void> =>
  when(store, (shown) => shown.view().hosts.get(hostKey)?.operation._tag === "Complete");
const failed = (store: Pick<Store, "view" | "subscribe">): Effect.Effect<void> =>
  when(store, (shown) => shown.view().hosts.get(hostKey)?.operation._tag === "Failed");

describe("failed deletion", () => {
  it.effect("forget closes the connection when deletion fails and retry removes the pairing", () =>
    Effect.scoped(
      Effect.gen(function* forgetting() {
        const page = yield* lifecyclePage();
        const store = yield* makeStore(page.boundary);
        store.pair(example.text);
        yield* when(store, (shown) => shown.view().hosts.get(hostKey)?.status.state === "Live");
        yield* page.denyRemoval(true);
        store.forget(hostKey);
        yield* failed(store);
        yield* Effect.gen(function* refused() {
          expect(yield* page.closed).toBe(1);
          expect(store.view().hosts.get(hostKey)?.fold.sessions.size).toBe(0);
          const sent = yield* Effect.result(
            store.send(hostKey, {
              Session: { Interrupt: { session: "00000000000000000000000001" } },
            }),
          );
          expect(Result.isFailure(sent)).toBe(true);
          store.reconnect();
          yield* Effect.yieldNow;
          expect(yield* page.connected).toBe(1);
        });
        yield* Effect.gen(function* retryingRemoval() {
          yield* page.denyRemoval(false);
          store.retry(hostKey);
          yield* when(store, (shown) => !shown.view().hosts.has(hostKey));
          expect(page.memory.getItem(recordKey) ?? undefined).toBeUndefined();
        });
      }),
    ),
  );
});

describe("failed join", () => {
  it.effect("a failed join retains its fragment and retry saves it before connecting", () =>
    Effect.scoped(
      Effect.gen(function* joining() {
        const fragment = `#join=${example.text}`;
        const page = yield* lifecyclePage(fragment);
        yield* page.denyWrites(true);
        const store = yield* makeStore(page.boundary);
        yield* failed(store);
        expect(page.at()).toBe(fragment);
        expect(yield* page.connected).toBe(0);
        yield* Effect.gen(function* retrying() {
          yield* page.denyWrites(false);
          store.retry(hostKey);
          yield* settled(store);
          expect(page.at()).toBe("#/");
          expect(restored(recordKey, page.memory.getItem(recordKey) ?? undefined)).toHaveLength(1);
          yield* when(store, (shown) => shown.view().hosts.get(hostKey)?.status.state === "Live");
          expect(yield* page.connected).toBe(1);
        });
      }),
    ),
  );
});

describe("abandoned pairing", () => {
  it.effect("forget waits for an older write without starting its abandoned pairing", () =>
    Effect.scoped(
      Effect.gen(function* abandoning() {
        const page = yield* lifecyclePage();
        const store = yield* makeStore(page.boundary);
        const write = yield* page.holdWrite;
        store.pair(example.text);
        yield* write.started;
        expect(store.view().hosts.get(hostKey)?.operation._tag).toBe("Pending");
        yield* Effect.gen(function* forgotten() {
          store.forget(hostKey);
          yield* write.release;
          yield* when(store, (shown) => !shown.view().hosts.has(hostKey));
          expect(yield* page.connected).toBe(0);
          expect(page.memory.getItem(recordKey) ?? undefined).toBeUndefined();
        });
      }),
    ),
  );
});

describe("superseded pairing", () => {
  it.effect("a later ticket is saved after an older pending write and owns the connection", () =>
    Effect.scoped(
      Effect.gen(function* replacing() {
        const page = yield* lifecyclePage();
        const store = yield* makeStore(page.boundary);
        const write = yield* page.holdWrite;
        store.pair(example.text);
        yield* write.started;
        yield* Effect.gen(function* replaced() {
          store.pair(renewed);
          yield* write.release;
          yield* settled(store);
          const records = restored(recordKey, page.memory.getItem(recordKey) ?? undefined);
          expect(records[0]?.ticket.token.slice(0, 4)).toStrictEqual([2, 2, 2, 1]);
          yield* when(store, (shown) => shown.view().hosts.get(hostKey)?.status.state === "Live");
          expect(yield* page.connected).toBe(1);
        });
      }),
    ),
  );
});

describe("failed replacement", () => {
  it.effect(
    "a failed ticket replacement is observable and retry starts its current connection",
    () =>
      Effect.scoped(
        Effect.gen(function* replacing() {
          const page = yield* lifecyclePage();
          const store = yield* makeStore(page.boundary);
          store.pair(example.text);
          yield* when(store, (shown) => shown.view().hosts.get(hostKey)?.status.state === "Live");
          yield* page.denyWrites(true);
          store.pair(renewed);
          yield* failed(store);
          yield* Effect.gen(function* retryingReplacement() {
            expect(yield* page.closed).toBe(1);
            expect(yield* page.connected).toBe(1);
            expect(store.view().hosts.get(hostKey)?.ticket.token.slice(0, 4)).toStrictEqual([
              2, 2, 2, 1,
            ]);
            yield* page.denyWrites(false);
            store.retry(hostKey);
            yield* settled(store);
            yield* when(store, (shown) => shown.view().hosts.get(hostKey)?.status.state === "Live");
            expect(yield* page.connected).toBe(2);
          });
        }),
      ),
  );
});

describe("saved hosts", () => {
  it.effect("restoring a saved host starts its connection without rewriting its pairing", () =>
    Effect.scoped(
      Effect.gen(function* restoring() {
        const page = yield* lifecyclePage();
        yield* Effect.scoped(
          Effect.gen(function* first() {
            const store = yield* makeStore(page.boundary);
            store.pair(example.text);
            yield* settled(store);
          }),
        );
        yield* page.denyWrites(true);
        const store = yield* makeStore(page.boundary);
        yield* when(store, (shown) => shown.view().hosts.get(hostKey)?.status.state === "Live");
        expect(store.view().hosts.get(hostKey)?.operation.action._tag).toBe("Remember");
      }),
    ),
  );
});

describe("delayed host names", () => {
  it.effect("a welcome-name write cannot overwrite a newer pairing", () =>
    Effect.scoped(
      Effect.gen(function* replacingAfterWelcome() {
        const page = yield* lifecyclePage();
        const connection = yield* page.holdConnection;
        const store = yield* makeStore(page.boundary);
        store.pair(example.text);
        yield* connection.started;
        const nameWrite = yield* page.holdWrite;
        yield* connection.release;
        yield* nameWrite.started;
        yield* Effect.gen(function* afterWelcome() {
          store.pair(renewed);
          yield* nameWrite.release;
          yield* settled(store);
          const [record] = restored(recordKey, page.memory.getItem(recordKey) ?? undefined);
          expect(record?.ticket.token.slice(0, 4)).toStrictEqual([2, 2, 2, 1]);
          expect(record?.name).toBe("test-host");
        });
      }),
    ),
  );
});

describe("forget after a host name", () => {
  it.effect("forget removes a pairing after an older welcome-name write completes", () =>
    Effect.scoped(
      Effect.gen(function* forgettingAfterWelcome() {
        const page = yield* lifecyclePage();
        const connection = yield* page.holdConnection;
        const store = yield* makeStore(page.boundary);
        store.pair(example.text);
        yield* connection.started;
        const nameWrite = yield* page.holdWrite;
        yield* connection.release;
        yield* nameWrite.started;
        yield* Effect.gen(function* afterWelcome() {
          store.forget(hostKey);
          yield* nameWrite.release;
          yield* when(store, (shown) => !shown.view().hosts.has(hostKey));
          expect(page.memory.getItem(recordKey) ?? undefined).toBeUndefined();
          expect(yield* page.closed).toBe(1);
          expect(yield* page.connected).toBe(1);
        });
      }),
    ),
  );
});

describe("pairing feedback", () => {
  it.effect("pairing renders pending and failed outcomes with a retry action", () =>
    Effect.scoped(
      Effect.gen(function* displaying() {
        const page = yield* lifecyclePage();
        const store = yield* makeStore(page.boundary);
        const write = yield* page.holdWrite;
        yield* page.denyWrites(true);
        store.pair(example.text);
        yield* write.started;
        yield* Effect.gen(function* feedback() {
          expect(renderToStaticMarkup(<Root store={store} />)).toContain("Saving pairing…");
          yield* write.release;
          yield* failed(store);
          const failure = renderToStaticMarkup(<Root store={store} />);
          expect(failure).toContain("The pairing could not be saved.");
          expect(failure).toContain('type="button">Retry</button>');
          yield* page.denyWrites(false);
          store.retry(hostKey);
          yield* settled(store);
          expect(page.at()).toBe("#/");
        });
      }),
    ),
  );
});

describe("pairing an existing host", () => {
  it.effect("the route follows the URL after pairing a completed ticket again", () =>
    Effect.scoped(
      Effect.gen(function* sameTicket() {
        const page = yield* lifecyclePage();
        const store = yield* makeStore(page.boundary);
        store.pair(example.text);
        yield* settled(store);
        store.show(Route.Pairing());
        store.pair(example.text);
        expect(page.at()).toBe("#/");
        expect(store.view().route).toStrictEqual(Route.Sessions());
      }),
    ),
  );
});

describe("restoring after Forget", () => {
  it.effect("a delayed restoration retry cannot revive a forgotten host", () =>
    Effect.scoped(
      Effect.gen(function* restoring() {
        const page = yield* lifecyclePage();
        yield* page.denyReads(true);
        const store = yield* makeStore(page.boundary);
        expect(store.view().restoration).toBe("Failed");
        store.pair(example.text);
        yield* settled(store);
        yield* page.denyReads(false);
        yield* Effect.gen(function* retryingRead() {
          const read = yield* page.holdRead;
          store.restore();
          yield* read.started;
          store.forget(hostKey);
          yield* when(store, (shown) => !shown.view().hosts.has(hostKey));
          yield* read.release;
          yield* when(store, (shown) => shown.view().restoration === "Ready");
          expect(store.view().hosts.has(hostKey)).toBe(false);
        });
      }),
    ),
  );
});
