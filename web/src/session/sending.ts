// Runs a screen's commands on its host's link and keeps what the person should be told.
import { Effect } from "effect";
import { useState } from "react";
import type { Command, CommandResult } from "../schemas/index.ts";
import { failed, notice, type Send } from "./commands.ts";

interface Sending {
  // What the last command's result or failure told the person, until the next command.
  readonly notice: string | null;
  readonly pending: boolean;
  readonly run: (send: Send, body: Command, taken?: (result: CommandResult) => void) => void;
}

const useSending = (): Sending => {
  const [told, setTold] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const run = (send: Send, body: Command, taken?: (result: CommandResult) => void): void => {
    setPending(true);
    setTold(null);
    Effect.runFork(
      send(body).pipe(
        Effect.match({
          onFailure: failed,
          onSuccess: (result) => {
            taken?.(result);
            return notice(result);
          },
        }),
        Effect.flatMap((next) =>
          Effect.sync(() => {
            setTold(next);
            setPending(false);
          }),
        ),
      ),
    );
  };
  return { notice: told, pending, run };
};

export { useSending };
