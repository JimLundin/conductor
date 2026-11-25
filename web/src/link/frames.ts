// Frames on the pipe's streams, each one JSON body that the generated schemas encode and decode.
import { Effect, Schema } from "effect";
import type { Connection, Stream } from "../pipe.ts";

const described = (error: unknown): string => {
  if (error instanceof Error) {
    return error.message;
  }
  return String(error);
};

const encoder = new TextEncoder();
const decoder = new TextDecoder();

const write = <T, E>(
  schema: Schema.Codec<T, E>,
  stream: Pick<Stream, "send">,
  frame: T,
): Effect.Effect<void, string> =>
  Schema.encodeEffect(schema)(frame).pipe(
    Effect.mapError(described),
    Effect.flatMap((json) =>
      Effect.tryPromise({
        try: () => stream.send(encoder.encode(JSON.stringify(json))),
        catch: described,
      }),
    ),
  );

const decode = <T>(schema: Schema.Decoder<T>, frame: Uint8Array): Effect.Effect<T, string> =>
  Effect.try({
    try: (): unknown => JSON.parse(decoder.decode(frame)),
    catch: described,
  }).pipe(
    Effect.flatMap((json) => Schema.decodeUnknownEffect(schema)(json)),
    Effect.mapError(described),
  );

// The stream's next frame, or undefined once the host finished it.
const read = <T>(
  schema: Schema.Decoder<T>,
  stream: Pick<Stream, "recv">,
): Effect.Effect<T | undefined, string> =>
  Effect.tryPromise({ try: () => stream.recv(), catch: described }).pipe(
    Effect.flatMap((frame) => {
      if (frame === undefined) {
        return Effect.succeed(undefined);
      }
      return decode(schema, frame);
    }),
  );

const open = (connection: Connection): Effect.Effect<Stream, string> =>
  Effect.tryPromise({ try: () => connection.openBi(), catch: described });

// Fails with the reason once the connection closes.
const closing = (connection: Connection): Effect.Effect<never, string> =>
  Effect.promise(() => connection.closed()).pipe(
    Effect.flatMap((reason) => Effect.fail(`the connection closed: ${reason}`)),
  );

const close = (connection: Connection): Effect.Effect<void> =>
  Effect.tryPromise(() => connection.close()).pipe(Effect.ignore);

export { close, closing, described, open, read, write };
