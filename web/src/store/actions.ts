import { Effect, FiberMap, Option, Result } from "effect";
import type { Pairing, StoreContext } from "#ui/store/types.ts";
import { Route, fragmentOf, isJoin } from "@conductor/core/route";
import { adoptHost, shown } from "@conductor/core/store";
import { remove, start } from "#ui/store/links.ts";
import { save, unsave } from "#ui/saved.ts";
import type { HostKey } from "@conductor/core/saved";
import type { TicketError } from "@conductor/core/ticket";
import type { View } from "@conductor/core/store";
import { parseJoin } from "@conductor/core/ticket";
import { replacedFragment } from "@conductor/core/runtime/fragment";

const withHosts: (context: StoreContext, hosts: View["hosts"]) => View = (context, hosts) => ({
  ...context.registry.get(context.state),
  hosts,
  route: shown(context.boundary.fragment(), hosts),
});

const adopt: (context: StoreContext, pairing: Pairing) => HostKey = (context, pairing) => {
  const view = context.registry.get(context.state);
  const planned = adoptHost(view, pairing.ticket, pairing.name);
  if (planned.host === undefined) {
    return planned.key;
  }
  context.registry.set(
    context.state,
    withHosts(context, new Map(view.hosts).set(planned.key, planned.host)),
  );
  const record = { name: planned.host.saved, ticket: planned.host.ticket };
  const started = start(context, planned.key, pairing.ticket);
  const persisted = save(context.boundary.storage, record).pipe(Effect.andThen(started));
  context.registry.set(context.execute, persisted);
  return planned.key;
};

const pairedFragment: (context: StoreContext, fragment: string) => void = (context, fragment) => {
  const joined = parseJoin(fragment);
  const notice = Option.getOrUndefined(Result.getFailure(joined));
  context.boundary.replace(replacedFragment(notice));
  if (Result.isSuccess(joined)) {
    adopt(context, { name: undefined, ticket: joined.success });
  }
  const view = context.registry.get(context.state);
  context.registry.set(context.state, {
    ...withHosts(context, view.hosts),
    notice,
  });
};

const follow: (context: StoreContext) => void = (context) => {
  const fragment = context.boundary.fragment();
  if (isJoin(fragment)) {
    pairedFragment(context, fragment);
    return;
  }
  const view = context.registry.get(context.state);
  context.registry.set(context.state, { ...view, route: shown(fragment, view.hosts) });
};

const show: (context: StoreContext, next: Route) => void = (context, next) => {
  context.boundary.go(fragmentOf(next));
  follow(context);
};

const pair: (context: StoreContext, text: string) => Result.Result<HostKey, TicketError> = (
  context,
  text,
) => {
  const parsed = parseJoin(text);
  if (Result.isFailure(parsed)) {
    return Result.fail(parsed.failure);
  }
  const key = adopt(context, { name: undefined, ticket: parsed.success });
  context.registry.update(context.state, (view) => ({ ...view, notice: undefined }));
  show(context, Route.Sessions());
  return Result.succeed(key);
};

const forget: (context: StoreContext, key: HostKey) => void = (context, key) => {
  remove(context, key);
  const stopped = FiberMap.remove(context.fibers, key);
  const removed = unsave(context.boundary.storage, key).pipe(Effect.andThen(stopped));
  context.registry.set(context.execute, removed);
  const view = context.registry.get(context.state);
  const kept = new Map([...view.hosts].filter(([paired]) => paired !== key));
  context.registry.set(context.state, withHosts(context, kept));
};

export { adopt, follow, forget, pair, show };
