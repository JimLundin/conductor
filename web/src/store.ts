import { Atom, AtomRegistry } from "effect/reactivity";
import type { Boundary, Store, StoreContext } from "#ui/store/types.ts";
import { Effect, FiberMap, FiberSet } from "effect";
import type { Scope, Semaphore } from "effect";
import { connect, load } from "#ui/store/hosts.ts";
import type { HostKey } from "@conductor/core/saved";
import type { HostLink } from "#ui/link/host.ts";
import { initialView } from "@conductor/core/store";
import { methods } from "#ui/store/methods.ts";

const openStore: (
  boundary: Boundary,
  registry: AtomRegistry.AtomRegistry,
) => Effect.Effect<Store, never, Scope.Scope> = (boundary, registry) =>
  Effect.gen(function* opening() {
    const fibers = yield* FiberMap.make<HostKey>();
    const run = yield* FiberSet.makeRuntime();
    const context: StoreContext = {
      boundary,
      connect: (host) => connect(context, host),
      execute: (effect): void => {
        run(effect);
      },
      fibers,
      linked: Atom.make<ReadonlyMap<HostKey, HostLink>>(new Map()).pipe(Atom.keepAlive),
      locks: Atom.make<ReadonlyMap<HostKey, Semaphore.Semaphore>>(new Map()).pipe(Atom.keepAlive),
      registry,
      state: Atom.make(initialView()).pipe(Atom.keepAlive),
    };
    yield* load(context);
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
