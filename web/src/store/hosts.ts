import type { Command, CommandResult, Ticket } from "@conductor/core/schemas/index";
import { Effect, FiberMap, Semaphore, Stream } from "effect";
import { HostAction, HostOperation, replacementFragment } from "@conductor/core/pairing";
import type { HostKey, Saved } from "@conductor/core/saved";
import type { PairedHost, Requested } from "@conductor/core/store";
import {
  adoptHost,
  applyPairedUpdate,
  rememberAction,
  requestHost,
  settledHost,
  shown,
} from "@conductor/core/store";
import { persist, restore } from "#ui/saved.ts";
import type { HostLink } from "#ui/link/host.ts";
import { SendError } from "@conductor/core/link";
import type { StoreContext } from "#ui/store/types.ts";
import type { Update } from "@conductor/core/link";
import { link } from "#ui/link.ts";

const oneOperation = 1;

const current = (context: StoreContext, host: PairedHost): boolean =>
  context.registry.get(context.state).hosts.get(host.key)?.operation.id === host.operation.id;

const remove = (context: StoreContext, key: HostKey): void => {
  context.registry.update(
    context.linked,
    (linked) => new Map([...linked].filter(([other]) => other !== key)),
  );
};

const linkedNow = (context: StoreContext, key: HostKey, active: HostLink): boolean =>
  context.registry.get(context.linked).get(key) === active;

const lock = (context: StoreContext, key: HostKey): Semaphore.Semaphore => {
  const locks = context.registry.get(context.locks);
  const known = locks.get(key);
  if (known !== undefined) {
    return known;
  }
  const created = Semaphore.makeUnsafe(oneOperation);
  context.registry.set(context.locks, new Map(locks).set(key, created));
  return created;
};

const prune = (context: StoreContext, key: HostKey, used: Semaphore.Semaphore): void => {
  const operation = context.registry.get(context.state).hosts.get(key)?.operation;
  const waiting = operation !== undefined && HostOperation.$is("Pending")(operation);
  if (!waiting && context.registry.get(context.locks).get(key) === used) {
    context.registry.update(
      context.locks,
      (locks) => new Map([...locks].filter(([other]) => other !== key)),
    );
  }
};

const serialized = (
  context: StoreContext,
  host: PairedHost,
  effect: Effect.Effect<void>,
): Effect.Effect<void> => {
  const used = lock(context, host.key);
  const pruned = Effect.sync(() => {
    prune(context, host.key, used);
  });
  return used
    .withPermit(Effect.suspend(() => (current(context, host) ? effect : Effect.void)))
    .pipe(Effect.ensuring(pruned));
};

const settle = (context: StoreContext, key: HostKey, operation: HostOperation): void => {
  context.registry.update(context.state, (view) => {
    const settled = settledHost(view, key, operation);
    if (settled === view) {
      return view;
    }
    return {
      ...settled,
      route: HostOperation.$is("Failed")(operation)
        ? settled.route
        : shown(context.boundary.fragment(), settled.hosts),
    };
  });
};

const finished = (context: StoreContext, host: PairedHost): void => {
  if (!current(context, host)) {
    return;
  }
  const fragment = replacementFragment(host.operation.action, context.boundary.fragment());
  if (fragment !== undefined) {
    context.boundary.replace(fragment);
  }
  const { action, id } = host.operation;
  settle(context, host.key, HostOperation.Complete({ action, id }));
};

const completed = (context: StoreContext, requested: Requested): Effect.Effect<void> =>
  Effect.gen(function* completing() {
    if (requested.steps.start && current(context, requested.host)) {
      yield* context.connect(requested.host);
    }
    finished(context, requested.host);
  });

const perform = (context: StoreContext, requested: Requested): Effect.Effect<void> =>
  persist(
    context.boundary.storage,
    { name: requested.host.saved, ticket: requested.host.ticket },
    requested.steps.storage,
  ).pipe(
    Effect.matchEffect({
      onFailure: () =>
        Effect.sync(() => {
          const { action, id } = requested.host.operation;
          settle(context, requested.host.key, HostOperation.Failed({ action, id }));
        }),
      onSuccess: () => completed(context, requested),
    }),
  );

const request = (context: StoreContext, requested: Requested): void => {
  context.registry.set(context.state, requested.view);
  const { host, steps } = requested;
  if (steps.stop) {
    remove(context, host.key);
  }
  const stopped = steps.stop ? FiberMap.remove(context.fibers, host.key) : Effect.void;
  const performed = serialized(context, host, perform(context, requested));
  context.execute(stopped.pipe(Effect.andThen(performed)));
};

const updated = (context: StoreContext, key: HostKey, update: Update): void => {
  const planned = applyPairedUpdate(context.registry.get(context.state), key, update);
  const host = planned.view.hosts.get(key);
  const action =
    planned.saved !== undefined && host !== undefined ? rememberAction(host) : undefined;
  if (action !== undefined && host !== undefined) {
    request(context, requestHost(planned.view, host, action));
    return;
  }
  context.registry.set(context.state, planned.view);
};

const follow = (context: StoreContext, key: HostKey, ticket: Ticket): Effect.Effect<void> =>
  Effect.scoped(
    Effect.gen(function* following() {
      const active = yield* link(context.boundary.pipe, ticket, context.boundary.client);
      context.registry.update(context.linked, (all) => new Map(all).set(key, active));
      yield* Effect.addFinalizer(() =>
        Effect.sync(() => {
          if (linkedNow(context, key, active)) {
            remove(context, key);
          }
        }),
      );
      yield* Stream.runForEach(active.updates, (update) =>
        Effect.sync(() => {
          if (linkedNow(context, key, active)) {
            updated(context, key, update);
          }
        }),
      );
    }),
  );

const connect = (context: StoreContext, host: PairedHost): Effect.Effect<void> =>
  FiberMap.run(context.fibers, host.key, follow(context, host.key, host.ticket)).pipe(
    Effect.asVoid,
  );

const requestFor = (context: StoreContext, host: PairedHost, action: HostAction): void => {
  request(context, requestHost(context.registry.get(context.state), host, action));
};

const adopt = (context: StoreContext, saved: Saved, action: HostAction): void => {
  const requested = adoptHost(context.registry.get(context.state), saved, action);
  if (requested !== undefined) {
    request(context, requested);
    return;
  }
  const fragment = replacementFragment(action, context.boundary.fragment());
  if (fragment !== undefined) {
    context.boundary.replace(fragment);
    context.registry.update(context.state, (view) => ({
      ...view,
      route: shown(fragment, view.hosts),
    }));
  }
};

const forget = (context: StoreContext, key: HostKey): void => {
  const host = context.registry.get(context.state).hosts.get(key);
  if (host !== undefined) {
    requestFor(context, host, HostAction.Forget());
  }
};

const retry = (context: StoreContext, key: HostKey): void => {
  const host = context.registry.get(context.state).hosts.get(key);
  if (host !== undefined && HostOperation.$is("Failed")(host.operation)) {
    requestFor(context, host, host.operation.action);
  }
};

const load = (context: StoreContext): Effect.Effect<void> =>
  restore(context.boundary.storage).pipe(
    Effect.match({
      onFailure: () => {
        context.registry.update(context.state, (view) => ({
          ...view,
          restoration: "Failed" as const,
        }));
      },
      onSuccess: (saved) => {
        for (const record of saved) {
          adopt(context, record, HostAction.Restore());
        }
        context.registry.update(context.state, (view) => ({
          ...view,
          restoration: "Ready" as const,
        }));
      },
    }),
  );

const send = (
  context: StoreContext,
  key: HostKey,
  body: Command,
): Effect.Effect<CommandResult, SendError> =>
  Effect.suspend(
    () =>
      context.registry.get(context.linked).get(key)?.send(body) ??
      Effect.fail(SendError.NotConnected()),
  );

const reconnect = (context: StoreContext): void => {
  const linked = context.registry.get(context.linked);
  context.execute(Effect.forEach(linked.values(), (value) => value.reconnect, { discard: true }));
};

export { adopt, connect, forget, load, reconnect, retry, send };
