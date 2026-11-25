import type { Command, CommandResult, Ticket } from "@conductor/core/schemas/index";
import { Effect, FiberMap, Semaphore, Stream } from "effect";
import { HostAction, HostOperation, replacementFragment } from "@conductor/core/pairing";
import type { HostKey, Saved } from "@conductor/core/saved";
import {
  adoptHost,
  applyPairedUpdate,
  rememberAction,
  requestedHost,
  settledHost,
  shown,
} from "@conductor/core/store";
import { restore, save, unsave } from "#ui/saved.ts";
import type { HostLink } from "#ui/link/host.ts";
import type { PairedHost } from "@conductor/core/store";
import { SendError } from "@conductor/core/link";
import type { StoreContext } from "#ui/store/types.ts";
import type { Update } from "@conductor/core/link";
import { link } from "#ui/link.ts";

const oneOperation = 1;

const current = (context: StoreContext, key: HostKey, action: HostAction): boolean =>
  context.registry.get(context.state).hosts.get(key)?.operation.action === action;

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

const settle = (context: StoreContext, key: HostKey, operation: HostOperation): void => {
  if (!current(context, key, operation.action)) {
    return;
  }
  context.registry.update(context.state, (view) => {
    const settled = settledHost(view, key, operation);
    return {
      ...settled,
      route: HostOperation.$is("Failed")(operation)
        ? settled.route
        : shown(context.boundary.fragment(), settled.hosts),
    };
  });
};

const remember = (context: StoreContext, host: PairedHost, action: HostAction): void => {
  const next = requestedHost(host, action);
  context.registry.update(context.state, (view) => ({
    ...view,
    hosts: new Map(view.hosts).set(host.key, next),
  }));
  const persisted = save(context.boundary.storage, { name: host.saved, ticket: host.ticket }).pipe(
    Effect.match({
      onFailure: () => {
        settle(context, host.key, HostOperation.Failed({ action }));
      },
      onSuccess: () => {
        settle(context, host.key, HostOperation.Complete({ action }));
      },
    }),
  );
  context.execute(
    lock(context, host.key).withPermit(
      Effect.suspend(() => (current(context, host.key, action) ? persisted : Effect.void)),
    ),
  );
};

const updated = (context: StoreContext, key: HostKey, update: Update): void => {
  const planned = applyPairedUpdate(context.registry.get(context.state), key, update);
  const host = planned.view.hosts.get(key);
  if (planned.saved !== undefined && host !== undefined) {
    const action = rememberAction(host);
    if (action !== undefined) {
      remember(context, host, action);
      return;
    }
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

const start = (context: StoreContext, host: PairedHost): Effect.Effect<void> =>
  FiberMap.run(context.fibers, host.key, follow(context, host.key, host.ticket)).pipe(
    Effect.asVoid,
  );

const completed = (
  context: StoreContext,
  host: PairedHost,
  action: HostAction,
): Effect.Effect<void> =>
  Effect.gen(function* completing() {
    if (!current(context, host.key, action)) {
      return;
    }
    if (HostAction.$is("Pair")(action) || HostAction.$is("Restore")(action)) {
      yield* start(context, host);
    }
    if (!current(context, host.key, action)) {
      return;
    }
    const fragment = replacementFragment(action, context.boundary.fragment());
    if (fragment !== undefined) {
      context.boundary.replace(fragment);
    }
    settle(context, host.key, HostOperation.Complete({ action }));
  });

const perform = (
  context: StoreContext,
  host: PairedHost,
  action: HostAction,
): Effect.Effect<void> => {
  const persist = HostAction.$match(action, {
    Forget: () => unsave(context.boundary.storage, host.key),
    Pair: () => save(context.boundary.storage, { name: host.saved, ticket: host.ticket }),
    Remember: () => save(context.boundary.storage, { name: host.saved, ticket: host.ticket }),
    Restore: () => Effect.void,
  });
  return persist.pipe(
    Effect.matchEffect({
      onFailure: () =>
        Effect.sync(() => {
          settle(context, host.key, HostOperation.Failed({ action }));
        }),
      onSuccess: () => completed(context, host, action),
    }),
  );
};

const request = (context: StoreContext, host: PairedHost, action: HostAction): void => {
  const next = requestedHost(host, action);
  context.registry.update(context.state, (view) => ({
    ...view,
    hosts: new Map(view.hosts).set(host.key, next),
  }));
  if (!current(context, host.key, action)) {
    return;
  }
  const retained = HostAction.$is("Remember")(action);
  if (!retained) {
    remove(context, host.key);
  }
  const stopped = retained ? Effect.void : FiberMap.remove(context.fibers, host.key);
  const serialized = lock(context, host.key).withPermit(
    Effect.suspend(() =>
      current(context, host.key, action) ? perform(context, next, action) : Effect.void,
    ),
  );
  context.execute(stopped.pipe(Effect.andThen(serialized)));
};

const adopt = (context: StoreContext, saved: Saved, action: HostAction): void => {
  const planned = adoptHost(context.registry.get(context.state), saved.ticket, saved.name);
  if (planned.host !== undefined) {
    request(context, planned.host, action);
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
    request(context, host, HostAction.Forget());
  }
};

const retry = (context: StoreContext, key: HostKey): void => {
  const host = context.registry.get(context.state).hosts.get(key);
  if (host !== undefined && HostOperation.$is("Failed")(host.operation)) {
    request(context, host, { ...host.operation.action });
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
          const { key } = adoptHost(
            context.registry.get(context.state),
            record.ticket,
            record.name,
          );
          if (!context.registry.get(context.locks).has(key)) {
            adopt(context, record, HostAction.Restore());
          }
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

export { adopt, forget, load, reconnect, retry, send };
