import { Route, fragmentOf } from "#core/route";
import { Data } from "effect";
import type { Unit } from "#core/data";

type HostAction = Data.TaggedEnum<{
  readonly Pair: { readonly fragment: string | undefined };
  readonly Forget: Unit;
  readonly Restore: Unit;
  readonly Remember: Unit;
}>;

const HostAction = Data.taggedEnum<HostAction>();

type HostOperation = Data.TaggedEnum<{
  readonly Pending: { readonly action: HostAction };
  readonly Failed: { readonly action: HostAction };
  readonly Complete: { readonly action: HostAction };
}>;

const HostOperation = Data.taggedEnum<HostOperation>();

const pendingText = HostAction.$match({
  Forget: () => "Forgetting host…",
  Pair: () => "Saving pairing…",
  Remember: () => "Saving host name…",
  Restore: () => "Restoring host…",
});

const failureText = HostAction.$match({
  Forget: () =>
    "Connection closed. The saved pairing could not be removed. Retry to forget this host.",
  Pair: () => "The pairing could not be saved. Retry to connect to this host.",
  Remember: () => "The host name could not be saved. The connection is still available.",
  Restore: () => "The host could not be restored.",
});

const operationText = HostOperation.$match({
  Complete: (): undefined => undefined,
  Failed: ({ action }) => failureText(action),
  Pending: ({ action }) => pendingText(action),
});

const replacementFragment = (action: HostAction, fragment: string): string | undefined =>
  HostAction.$is("Pair")(action) && action.fragment === fragment
    ? fragmentOf(Route.Sessions())
    : undefined;

export { HostAction, HostOperation, operationText, replacementFragment };
