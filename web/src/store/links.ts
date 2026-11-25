import type { Command, CommandResult, Ticket } from "@conductor/core/schemas/index";
import { Effect, FiberMap, Stream } from "effect";
import type { HostKey } from "@conductor/core/saved";
import type { HostLink } from "#ui/link/host.ts";
import { SendError } from "@conductor/core/link";
import type { StoreContext } from "#ui/store/types.ts";
import type { Update } from "@conductor/core/link";
import { applyPairedUpdate } from "@conductor/core/store";
import { link } from "#ui/link.ts";
import { save } from "#ui/saved.ts";

const updated: (context: StoreContext, key: HostKey, update: Update) => void = (
  context,
  key,
  update,
) => {
  const planned = applyPairedUpdate(context.registry.get(context.state), key, update);
  context.registry.set(context.state, planned.view);
  if (planned.saved !== undefined) {
    context.registry.set(context.execute, save(context.boundary.storage, planned.saved));
  }
};

const current: (context: StoreContext, key: HostKey, linked: HostLink) => boolean = (
  context,
  key,
  linked,
) => context.registry.get(context.linked).get(key) === linked;

const remove: (context: StoreContext, key: HostKey) => void = (context, key) => {
  context.registry.update(
    context.linked,
    (linked) => new Map([...linked].filter(([other]) => other !== key)),
  );
};

const forward: (context: StoreContext, key: HostKey, linked: HostLink) => Effect.Effect<void> = (
  context,
  key,
  linked,
) =>
  Stream.runForEach(linked.updates, (update) =>
    Effect.sync(() => {
      if (current(context, key, linked)) {
        updated(context, key, update);
      }
    }),
  );

const follow: (context: StoreContext, key: HostKey, ticket: Ticket) => Effect.Effect<void> = (
  context,
  key,
  ticket,
) =>
  Effect.scoped(
    Effect.gen(function* following() {
      const linked = yield* link(context.boundary.pipe, ticket, context.boundary.client);
      context.registry.update(context.linked, (all) => new Map(all).set(key, linked));
      yield* Effect.addFinalizer(() =>
        Effect.sync(() => {
          if (current(context, key, linked)) {
            remove(context, key);
          }
        }),
      );
      yield* forward(context, key, linked);
    }),
  );

const start: (context: StoreContext, key: HostKey, ticket: Ticket) => Effect.Effect<void> = (
  context,
  key,
  ticket,
) => FiberMap.run(context.fibers, key, follow(context, key, ticket)).pipe(Effect.asVoid);

const send: (
  context: StoreContext,
  key: HostKey,
  body: Command,
) => Effect.Effect<CommandResult, SendError> = (context, key, body) =>
  Effect.suspend(
    () =>
      context.registry.get(context.linked).get(key)?.send(body) ??
      Effect.fail(SendError.NotConnected()),
  );

const reconnect: (context: StoreContext) => void = (context) => {
  const linked = context.registry.get(context.linked);
  context.registry.set(
    context.execute,
    Effect.forEach(linked.values(), (value) => value.reconnect, { discard: true }),
  );
};

export { reconnect, remove, send, start };
