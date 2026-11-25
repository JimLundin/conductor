import { Array as Arr, Effect } from "effect";
import type { Boundary, Store } from "#ui/store.ts";
import { KeyValueStore } from "effect/persistence";
import type { Pipe } from "#ui/pipe.ts";
import { dialing } from "#ui/test/replay.ts";

const memoryStorage = (entries: ReadonlyArray<readonly [string, string]> = []): Storage => {
  const items = new Map(entries);
  return {
    clear: (): void => {
      items.clear();
    },
    getItem: (key: string): string | null => items.get(key) ?? null,
    key: (index: number): string | null => [...items.keys()][index] ?? null,
    get length(): number {
      return items.size;
    },
    removeItem: (key: string): void => {
      items.delete(key);
    },
    setItem: (key: string, value: string): void => {
      items.set(key, value);
    },
  };
};

const keysOf = (storage: Readonly<Storage>): ReadonlyArray<string> =>
  storage.length === 0 ? [] : Arr.makeBy(storage.length, (index) => storage.key(index) ?? "");

interface Page {
  readonly boundary: Boundary;
  readonly at: () => string;
}

const page = (
  start: string,
  storage: Readonly<Storage> = memoryStorage(),
  pipe: Pipe = dialing([]),
): Page => {
  let fragment = start;
  const move = (next: string): void => {
    fragment = next;
  };
  return {
    at: (): string => fragment,
    boundary: {
      client: "test-client",
      fragment: (): string => fragment,
      go: move,
      pipe,
      replace: move,
      storage: Effect.runSync(
        Effect.provide(
          KeyValueStore.KeyValueStore,
          KeyValueStore.layerStorage(() => storage),
        ),
      ),
    },
  };
};

const when: (
  store: Pick<Store, "view" | "subscribe">,
  holds: (store: Pick<Store, "view">) => boolean,
) => Effect.Effect<void> = (store, holds) =>
  Effect.callback((resume) => {
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

export { keysOf, memoryStorage, page, when };
