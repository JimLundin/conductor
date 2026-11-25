import { AsyncResult, Atom } from "effect/reactivity";
import type { Command, CommandResult } from "@conductor/core/schemas/index";
import { Effect, Option } from "effect";
import { failed, notice } from "@conductor/core/session/commands";
import type { Send } from "#ui/link/host.ts";
import { useAtom } from "@effect/atom-react";
import { useMemo } from "react";

interface Submission {
  readonly body: Command;
  readonly send: Send;
  readonly taken: ((result: CommandResult) => void) | undefined;
}

interface Sending {
  readonly notice: string | undefined;
  readonly pending: boolean;
  readonly run: (send: Send, body: Command, taken?: (result: CommandResult) => void) => void;
}

const submit: (submission: Submission) => Effect.Effect<string | undefined> = (submission) =>
  submission.send(submission.body).pipe(
    Effect.tap((result) => Effect.sync(() => submission.taken?.(result))),
    Effect.match({ onFailure: failed, onSuccess: notice }),
  );

const useSending: () => Sending = () => {
  const sent = useMemo(() => Atom.fn(submit), []);
  const [result, run] = useAtom(sent);
  return {
    notice: result.waiting ? undefined : Option.getOrUndefined(AsyncResult.value(result)),
    pending: result.waiting,
    run: (send, body, taken): void => {
      run({ body, send, taken });
    },
  };
};

export { useSending };
