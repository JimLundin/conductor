import type { Atom, AtomRegistry } from "effect/reactivity";
import type { Command, CommandResult } from "@conductor/core/schemas/index";
import type { Effect, FiberMap, Semaphore } from "effect";
import type { Boundary } from "#ui/store/boundary.ts";
import type { HostKey } from "@conductor/core/saved";
import type { HostLink } from "#ui/link/host.ts";
import type { Route } from "@conductor/core/route";
import type { SendError } from "@conductor/core/link";
import type { View } from "@conductor/core/store";

interface Store {
  readonly follow: () => void;
  readonly forget: (host: HostKey) => void;
  readonly pair: (text: string) => void;
  readonly retry: (host: HostKey) => void;
  readonly restore: () => void;
  readonly reconnect: () => void;
  readonly registry: AtomRegistry.AtomRegistry;
  readonly send: (host: HostKey, body: Command) => Effect.Effect<CommandResult, SendError>;
  readonly show: (route: Route) => void;
  readonly subscribe: (changed: () => void) => () => void;
  readonly view: () => View;
  readonly viewAtom: Atom.Atom<View>;
}

interface StoreContext {
  readonly boundary: Boundary;
  readonly execute: (effect: Effect.Effect<void>) => void;
  readonly locks: Atom.Writable<ReadonlyMap<HostKey, Semaphore.Semaphore>>;
  readonly fibers: FiberMap.FiberMap<HostKey>;
  readonly linked: Atom.Writable<ReadonlyMap<HostKey, HostLink>>;
  readonly registry: AtomRegistry.AtomRegistry;
  readonly state: Atom.Writable<View>;
}

export type { Store, StoreContext };
export type { Boundary } from "#ui/store/boundary.ts";
