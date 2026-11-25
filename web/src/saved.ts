import type { HostKey, Saved } from "@conductor/core/saved";
import { hostKey, restored, savedText, storageKey } from "@conductor/core/saved";
import { Effect } from "effect";
import type { KeyValueStore } from "effect/persistence";

const restore: (storage: KeyValueStore.KeyValueStore) => Effect.Effect<ReadonlyArray<Saved>> = (
  storage,
) =>
  storage.keys.pipe(
    Effect.flatMap((keys) =>
      Effect.forEach(keys, (key) =>
        storage.get(key).pipe(Effect.map((value) => restored(key, value))),
      ),
    ),
    Effect.map((records) => records.flat()),
    Effect.orDie,
  );

const save: (storage: KeyValueStore.KeyValueStore, saved: Saved) => Effect.Effect<void> = (
  storage,
  saved,
) => storage.set(storageKey(hostKey(saved.ticket)), savedText(saved)).pipe(Effect.orDie);

const unsave: (storage: KeyValueStore.KeyValueStore, host: HostKey) => Effect.Effect<void> = (
  storage,
  host,
) => storage.remove(storageKey(host)).pipe(Effect.orDie);

export { restore, save, unsave };
