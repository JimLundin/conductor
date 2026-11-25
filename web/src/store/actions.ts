import { Option, Result } from "effect";
import { Route, fragmentOf, isJoin } from "@conductor/core/route";
import { HostAction } from "@conductor/core/pairing";
import type { StoreContext } from "#ui/store/types.ts";
import { adopt } from "#ui/store/hosts.ts";
import { parseJoin } from "@conductor/core/ticket";
import { replacedFragment } from "@conductor/core/runtime/fragment";
import { shown } from "@conductor/core/store";

const follow = (context: StoreContext): void => {
  const fragment = context.boundary.fragment();
  if (isJoin(fragment)) {
    const joined = parseJoin(fragment);
    const notice = Option.getOrUndefined(Result.getFailure(joined));
    context.registry.update(context.state, (view) => ({ ...view, notice, route: Route.Pairing() }));
    if (Result.isSuccess(joined)) {
      adopt(context, { name: undefined, ticket: joined.success }, HostAction.Pair({ fragment }));
    } else {
      context.boundary.replace(replacedFragment(notice));
    }
    return;
  }
  context.registry.update(context.state, (view) => ({
    ...view,
    route: shown(fragment, view.hosts),
  }));
};

const show = (context: StoreContext, next: Route): void => {
  context.boundary.go(fragmentOf(next));
  follow(context);
};

const pair = (context: StoreContext, text: string): void => {
  const parsed = parseJoin(text);
  const notice = Option.getOrUndefined(Result.getFailure(parsed));
  context.registry.update(context.state, (view) => ({ ...view, notice }));
  if (Result.isSuccess(parsed)) {
    adopt(
      context,
      { name: undefined, ticket: parsed.success },
      HostAction.Pair({ fragment: context.boundary.fragment() }),
    );
  }
};

export { follow, pair, show };
