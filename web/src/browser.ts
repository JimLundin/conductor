import type { Boundary, Store } from "#ui/store.ts";
import { BrowserKeyValueStore, BrowserStream } from "@effect/platform-browser";
import { Effect, Stream } from "effect";
import init, { connect } from "#transport";
import { Atom } from "effect/reactivity";
import { KeyValueStore } from "effect/persistence";
import { deviceName } from "@conductor/core/device";
import { isVisible } from "@conductor/core/runtime/browser";
import { makeStore } from "#ui/store.ts";

const boundary: (storage: KeyValueStore.KeyValueStore) => Boundary = (storage) => ({
  client: deviceName(navigator.userAgent, navigator.maxTouchPoints),
  fragment: () => location.hash,
  go: (fragment): void => {
    location.hash = fragment;
  },
  pipe: { connect },
  replace: (fragment): void => {
    history.replaceState(undefined, "", fragment);
  },
  storage,
});

const listen: (store: Store) => Effect.Effect<void> = (store) =>
  Effect.all(
    [
      BrowserStream.fromEventListenerWindow("hashchange").pipe(
        Stream.runForEach(() => Effect.sync(store.follow)),
      ),
      BrowserStream.fromEventListenerWindow("online").pipe(
        Stream.runForEach(() => Effect.sync(store.reconnect)),
      ),
      BrowserStream.fromEventListenerDocument("visibilitychange").pipe(
        Stream.filter((event) => isVisible(event.target)),
        Stream.runForEach(() => Effect.sync(store.reconnect)),
      ),
    ],
    { concurrency: "unbounded", discard: true },
  );

const browser = Atom.runtime(BrowserKeyValueStore.layerLocalStorage);

const application = browser.atom((get) =>
  Effect.gen(function* opening() {
    yield* Effect.tryPromise(() => init()).pipe(Effect.orDie);
    const storage = yield* KeyValueStore.KeyValueStore;
    const store = yield* makeStore(boundary(storage), get.registry);
    yield* Effect.forkScoped(listen(store));
    return store;
  }),
);

export { application };
