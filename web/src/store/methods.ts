import { Atom } from "effect/reactivity";
import { Option } from "effect";
import type { Result } from "effect";
import type { HostKey } from "@conductor/core/saved";
import type { Route } from "@conductor/core/route";
import type { TicketError } from "@conductor/core/ticket";
import { follow, forget, pair, show } from "#ui/store/actions.ts";
import { reconnect, send } from "#ui/store/links.ts";
import type { Store, StoreContext } from "#ui/store/types.ts";

const methods: (context: StoreContext) => Store = (context) => {
  const followed = Atom.fnSync(() =>{  follow(context); });
  const shown = Atom.fnSync((route: Route) =>{  show(context, route); });
  const forgotten = Atom.fnSync((key: HostKey) =>{  forget(context, key); });
  const paired = Atom.fnSync((text: string) => pair(context, text));
  const reconnected = Atom.fnSync(() =>{  reconnect(context); });
  const pairing: (text: string) => Result.Result<HostKey, TicketError> = (text) => {
    context.registry.set(paired, text);
    return Option.getOrThrow(context.registry.get(paired));
  };
  return {
    follow: () =>{  context.registry.set(followed, undefined); },
    forget: (key) =>{  context.registry.set(forgotten, key); },
    pair: pairing,
    reconnect: () =>{  context.registry.set(reconnected, undefined); },
    registry: context.registry,
    send: (key, body) => send(context, key, body),
    show: (route) =>{  context.registry.set(shown, route); },
    subscribe: (changed) => context.registry.subscribe(context.state, changed),
    view: () => context.registry.get(context.state),
    viewAtom: context.state,
  };
};

export { methods };
