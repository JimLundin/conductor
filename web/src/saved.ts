import type { HostKey, Saved } from "@conductor/core/saved";
import { hostKey, restored, savedText, storageKey } from "@conductor/core/saved";
import { Effect } from "effect";
import type { KeyValueStore } from "effect/persistence";
import type { Steps } from "@conductor/core/pairing";

const restore: (
  storage: KeyValueStore.KeyValueStore,
) => Effect.Effect<ReadonlyArray<Saved>, KeyValueStore.KeyValueStoreError> = (storage) =>
  storage.keys.pipe(
    Effect.flatMap((keys) =>
      Effect.forEach(keys, (key) =>
        storage.get(key).pipe(Effect.map((value) => restored(key, value))),
      ),
    ),
    Effect.map((records) => records.flat()),
  );

const save: (
  storage: KeyValueStore.KeyValueStore,
  saved: Saved,
) => Effect.Effect<void, KeyValueStore.KeyValueStoreError> = (storage, saved) =>
  storage.set(storageKey(hostKey(saved.ticket)), savedText(saved));

const unsave: (
  storage: KeyValueStore.KeyValueStore,
  host: HostKey,
) => Effect.Effect<void, KeyValueStore.KeyValueStoreError> = (storage, host) =>
  storage.remove(storageKey(host));

const persist: (
  storage: KeyValueStore.KeyValueStore,
  saved: Saved,
  step: Steps["storage"],
) => Effect.Effect<void, KeyValueStore.KeyValueStoreError> = (storage, saved, step) =>
  ({
    Keep: Effect.void,
    Remove: unsave(storage, hostKey(saved.ticket)),
    Save: save(storage, saved),
  })[step];

export { persist, restore };
