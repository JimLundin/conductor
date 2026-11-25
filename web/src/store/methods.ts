import type { Store, StoreContext } from "#ui/store/types.ts";
import { follow, pair, show } from "#ui/store/actions.ts";
import { forget, load, reconnect, retry, send } from "#ui/store/hosts.ts";
import { Atom } from "effect/reactivity";
import type { HostKey } from "@conductor/core/saved";
import type { Route } from "@conductor/core/route";

const methods: (context: StoreContext) => Store = (context) => {
  const followed = Atom.fnSync(() => {
    follow(context);
  });
  const shown = Atom.fnSync((route: Route) => {
    show(context, route);
  });
  const forgotten = Atom.fnSync((key: HostKey) => {
    forget(context, key);
  });
  const retried = Atom.fnSync((key: HostKey) => {
    retry(context, key);
  });
  const paired = Atom.fnSync((text: string) => {
    pair(context, text);
  });
  const reconnected = Atom.fnSync(() => {
    reconnect(context);
  });
  return {
    follow: (): void => {
      context.registry.set(followed, undefined);
    },
    forget: (key): void => {
      context.registry.set(forgotten, key);
    },
    pair: (text): void => {
      context.registry.set(paired, text);
    },
    reconnect: (): void => {
      context.registry.set(reconnected, undefined);
    },
    registry: context.registry,
    restore: (): void => {
      context.execute(load(context));
    },
    retry: (key): void => {
      context.registry.set(retried, key);
    },
    send: (key, body) => send(context, key, body),
    show: (route): void => {
      context.registry.set(shown, route);
    },
    subscribe: (changed) => context.registry.subscribe(context.state, changed),
    view: () => context.registry.get(context.state),
    viewAtom: context.state,
  };
};

export { methods };
