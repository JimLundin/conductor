// A command on its own stream, with its result for the caller.
import { CommandResult, Open } from "@conductor/core/schemas/index";
import { closing, described, open, read, write } from "#ui/link/frames.ts";
import { BrowserCrypto } from "@effect/platform-browser";
import type { Command } from "@conductor/core/schemas/index";
import type { Connection } from "#ui/pipe.ts";
import { Effect } from "effect";
import { SecureRandom } from "effect/crypto";
import { SendError } from "@conductor/core/link";

const answer: (
  connection: Connection,
  body: Command,
) => Effect.Effect<CommandResult | undefined, string> = (connection, body) =>
  Effect.gen(function* rpc() {
    const stream = yield* open(connection);
    const random = yield* SecureRandom.SecureRandom;
    const id = yield* random.uuidV4();
    yield* write(Open, stream, { Command: { body, id } });
    yield* Effect.tryPromise({ catch: described, try: () => stream.finish() });
    return yield* read(CommandResult, stream);
  }).pipe(Effect.provide(BrowserCrypto.layerSecureRandom));

const resulted: (result: CommandResult | undefined) => Effect.Effect<CommandResult, SendError> = (
  result,
) => {
  if (result === undefined) {
    return Effect.fail(SendError.NoResult());
  }
  return Effect.succeed(result);
};

const command: (
  connection: Connection,
  body: Command,
) => Effect.Effect<CommandResult, SendError> = (connection, body) =>
  Effect.raceFirst(answer(connection, body), closing(connection)).pipe(
    Effect.mapError((reason) => SendError.Wire({ reason })),
    Effect.flatMap(resulted),
  );

export { command };
