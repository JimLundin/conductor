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

interface Attempt {
  readonly id: number;
  readonly action: HostAction;
}

type HostOperation = Data.TaggedEnum<{
  readonly Pending: Attempt;
  readonly Failed: Attempt;
  readonly Complete: Attempt;
}>;

const HostOperation = Data.taggedEnum<HostOperation>();

interface Steps {
  readonly stop: boolean;
  readonly start: boolean;
  readonly storage: "Save" | "Remove" | "Keep";
}

const stepsOf = HostAction.$match({
  Forget: (): Steps => ({ start: false, stop: true, storage: "Remove" }),
  Pair: (): Steps => ({ start: true, stop: true, storage: "Save" }),
  Remember: (): Steps => ({ start: false, stop: false, storage: "Save" }),
  Restore: (): Steps => ({ start: true, stop: true, storage: "Keep" }),
});

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
  Restore: (): undefined => undefined,
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

export { HostAction, HostOperation, operationText, replacementFragment, stepsOf };
export type { Attempt, Steps };
