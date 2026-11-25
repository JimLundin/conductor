import { Atom, AtomRegistry } from "effect/reactivity";
import type { Boundary, Store, StoreContext } from "#ui/store/types.ts";
import { Effect, FiberMap } from "effect";
import type { HostKey } from "@conductor/core/saved";
import type { HostLink } from "#ui/link/host.ts";
import type { Scope } from "effect";
import { adopt } from "#ui/store/actions.ts";
import { initialView } from "@conductor/core/store";
import { methods } from "#ui/store/methods.ts";
import { restore } from "#ui/saved.ts";

const openStore: (
  boundary: Boundary,
  registry: AtomRegistry.AtomRegistry,
) => Effect.Effect<Store, never, Scope.Scope> = (boundary, registry) =>
  Effect.gen(function* opening() {
    const execute = Atom.fn((effect: Effect.Effect<void>) => effect, { concurrent: true }).pipe(
      Atom.keepAlive,
    );
    const context: StoreContext = {
      boundary,
      execute,
      fibers: yield* FiberMap.make<HostKey>(),
      linked: Atom.make<ReadonlyMap<HostKey, HostLink>>(new Map()).pipe(Atom.keepAlive),
      registry,
      state: Atom.make(initialView()).pipe(Atom.keepAlive),
    };
    const saved = yield* restore(boundary.storage);
    const adopted = Atom.fnSync((record: (typeof saved)[number]) =>
      adopt(context, { name: record.name, ticket: record.ticket }),
    );
    yield* Effect.forEach(
      saved,
      (record) =>
        Effect.sync(() => {
          registry.set(adopted, record);
        }),
      { discard: true },
    );
    const store = methods(context);
    store.follow();
    return store;
  });

const ownedRegistry = Effect.acquireRelease(
  Effect.sync(() => AtomRegistry.make()),
  (registry) =>
    Effect.sync(() => {
      registry.dispose();
    }),
);

const makeStore: (
  boundary: Boundary,
  registry?: AtomRegistry.AtomRegistry,
) => Effect.Effect<Store, never, Scope.Scope> = (boundary, registry) =>
  (registry === undefined ? ownedRegistry : Effect.succeed(registry)).pipe(
    Effect.flatMap((active) => openStore(boundary, active)),
  );

export { makeStore };
export type { Boundary, Store } from "#ui/store/types.ts";
