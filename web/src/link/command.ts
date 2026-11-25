// A command on its own stream, with its result for the caller.
import { Data, Effect } from "effect";
import type { Connection } from "../pipe.ts";
import { type Command, CommandResult, Open } from "../schemas/index.ts";
import { closing, described, open, read, write } from "./frames.ts";

type SendError = Data.TaggedEnum<{
  readonly NotConnected: Record<never, never>;
  readonly NoResult: Record<never, never>;
  readonly Wire: { readonly reason: string };
}>;

const SendError = Data.taggedEnum<SendError>();

const answer = (
  connection: Connection,
  body: Command,
): Effect.Effect<CommandResult | undefined, string> =>
  Effect.gen(function* rpc() {
    const stream = yield* open(connection);
    yield* write(Open, stream, { Command: { id: crypto.randomUUID(), body } });
    yield* Effect.tryPromise({ try: () => stream.finish(), catch: described });
    return yield* read(CommandResult, stream);
  });

const resulted = (result: CommandResult | undefined): Effect.Effect<CommandResult, SendError> => {
  if (result === undefined) {
    return Effect.fail(SendError.NoResult());
  }
  return Effect.succeed(result);
};

const command = (connection: Connection, body: Command): Effect.Effect<CommandResult, SendError> =>
  Effect.raceFirst(answer(connection, body), closing(connection)).pipe(
    Effect.mapError((reason) => SendError.Wire({ reason })),
    Effect.flatMap(resulted),
  );

export { command, SendError };
