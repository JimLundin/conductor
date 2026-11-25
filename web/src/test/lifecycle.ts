import type { Connection, Pipe } from "#ui/pipe.ts";
import { Deferred, Effect, FiberSet, Option, Ref } from "effect";
import { memoryStorage, page } from "#ui/test/page.ts";
import { scenario, serve } from "#ui/test/replay.ts";
import type { Boundary } from "#ui/store.ts";
import { KeyValueStore } from "effect/persistence";
import type { Scope } from "effect";
import { fakeHost } from "#ui/test/fake-pipe.ts";

interface Release {
  readonly started: Effect.Effect<void>;
  readonly release: Effect.Effect<void>;
}

interface Gate {
  readonly wait: Effect.Effect<void>;
  readonly hold: Effect.Effect<Release>;
}

interface Waiting {
  readonly started: Deferred.Deferred<boolean>;
  readonly released: Deferred.Deferred<boolean>;
}

const gate: () => Effect.Effect<Gate> = () =>
  Effect.gen(function* gating() {
    const next = yield* Ref.make(Option.none<Waiting>());
    return {
      hold: Effect.gen(function* holding() {
        const waiting: Waiting = {
          released: yield* Deferred.make<boolean>(),
          started: yield* Deferred.make<boolean>(),
        };
        yield* Ref.set(next, Option.some(waiting));
        return {
          release: Deferred.succeed(waiting.released, true).pipe(Effect.asVoid),
          started: Deferred.await(waiting.started).pipe(Effect.asVoid),
        };
      }),
      wait: Ref.getAndSet(next, Option.none()).pipe(
        Effect.flatMap(
          Option.match({
            onNone: () => Effect.void,
            onSome: (waiting) =>
              Deferred.succeed(waiting.started, true).pipe(
                Effect.andThen(Deferred.await(waiting.released)),
                Effect.asVoid,
              ),
          }),
        ),
      ),
    };
  });

interface Stored {
  readonly memory: Readonly<Storage>;
  readonly service: KeyValueStore.KeyValueStore;
  readonly denyReads: (denied: boolean) => Effect.Effect<void>;
  readonly holdRead: Effect.Effect<Release>;
  readonly denyRemoval: (denied: boolean) => Effect.Effect<void>;
  readonly denyWrites: (denied: boolean) => Effect.Effect<void>;
  readonly holdWrite: Effect.Effect<Release>;
}

interface Permission {
  readonly check: Effect.Effect<void, KeyValueStore.KeyValueStoreError>;
  readonly deny: (denied: boolean) => Effect.Effect<void>;
}

const permission = (method: string): Effect.Effect<Permission> =>
  Effect.gen(function* checking() {
    const denied = yield* Ref.make(false);
    return {
      check: Ref.get(denied).pipe(
        Effect.flatMap((failed) =>
          failed
            ? Effect.fail(new KeyValueStore.KeyValueStoreError({ message: "denied", method }))
            : Effect.void,
        ),
      ),
      deny: (failed): Effect.Effect<void> => Ref.set(denied, failed),
    };
  });

const stored: () => Effect.Effect<Stored> = () =>
  Effect.gen(function* storage() {
    const memory = memoryStorage();
    const original = page("", memory).boundary.storage;
    const read = yield* gate();
    const write = yield* gate();
    const keys = yield* permission("keys");
    const remove = yield* permission("remove");
    const set = yield* permission("set");
    const service: KeyValueStore.KeyValueStore = {
      ...original,
      get: (key) => original.get(key).pipe(Effect.tap(() => read.wait)),
      keys: keys.check.pipe(Effect.andThen(original.keys)),
      remove: (key) => remove.check.pipe(Effect.andThen(original.remove(key))),
      set: (key, value: Readonly<Uint8Array> | string) =>
        write.wait.pipe(Effect.andThen(set.check), Effect.andThen(original.set(key, value))),
    };
    return {
      denyReads: keys.deny,
      denyRemoval: remove.deny,
      denyWrites: set.deny,
      holdRead: read.hold,
      holdWrite: write.hold,
      memory,
      service,
    };
  });

interface Connected {
  readonly pipe: Pipe;
  readonly closed: Effect.Effect<number>;
  readonly connected: Effect.Effect<number>;
  readonly holdConnection: Effect.Effect<Release>;
}

const connected: () => Effect.Effect<Connected, never, Scope.Scope> = () =>
  Effect.gen(function* connections() {
    const closed = yield* Ref.make(0);
    const count = yield* Ref.make(0);
    const connectionGate = yield* gate();
    const run = yield* FiberSet.makeRuntime();
    const welcome = yield* scenario("a_claude_turn_streams_asks_and_finishes").welcome(1);
    const connect: Pipe["connect"] = () =>
      Effect.runPromise(
        Effect.gen(function* connecting() {
          yield* connectionGate.wait;
          const fake = yield* Effect.promise(fakeHost);
          yield* Ref.update(count, (value) => value + 1);
          const serving = serve(fake, {
            feed: [{ Instances: welcome.instances }],
            reply: { Welcome: { ...welcome, head_seq: 0 } },
            results: [],
          });
          const served = serving.pipe(Effect.andThen(Effect.never), Effect.scoped);
          run(served);
          const connection = yield* Effect.promise(() =>
            fake.pipe.connect(new Uint8Array(), "", new Uint8Array()),
          );
          const closing = Ref.update(closed, (value) => value + 1).pipe(
            Effect.andThen(Effect.promise(() => connection.close())),
          );
          const observed: Connection = {
            [Symbol.dispose]: (): void => {
              connection[Symbol.dispose]();
            },
            close: (): Promise<void> => Effect.runPromise(closing),
            closed: (): Promise<string> => connection.closed(),
            free: (): void => {
              connection.free();
            },
            openBi: () => connection.openBi(),
          };
          return observed;
        }),
      );
    return {
      closed: Ref.get(closed),
      connected: Ref.get(count),
      holdConnection: connectionGate.hold,
      pipe: { connect },
    };
  });

interface LifecyclePage extends Omit<Stored, "service">, Omit<Connected, "pipe"> {
  readonly at: () => string;
  readonly boundary: Boundary;
}

const lifecyclePage = (fragment = ""): Effect.Effect<LifecyclePage, never, Scope.Scope> =>
  Effect.gen(function* fixture() {
    const storage = yield* stored();
    const connections = yield* connected();
    const original = page(fragment, storage.memory);
    return {
      ...storage,
      ...connections,
      at: original.at,
      boundary: { ...original.boundary, pipe: connections.pipe, storage: storage.service },
    };
  });

export { lifecyclePage };
