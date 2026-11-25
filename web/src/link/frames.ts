import type { Connection, Stream } from "#ui/pipe.ts";
import { Effect, Predicate, Schema } from "effect";

const described: (error: unknown) => string = (error) => {
  if (Predicate.isError(error)) {
    return error.message;
  }
  return String(error);
};

const write: <Decoded, Encoded>(
  schema: Schema.Codec<Decoded, Encoded>,
  stream: Pick<Stream, "send">,
  frame: Decoded,
) => Effect.Effect<void, string> = (schema, stream, frame) =>
  Schema.encodeEffect(Schema.fromJsonString(schema))(frame).pipe(
    Effect.mapError(described),
    Effect.flatMap((json) =>
      Effect.tryPromise({
        catch: described,
        try: () => stream.send(new TextEncoder().encode(json)),
      }),
    ),
  );

const decode: <Decoded>(
  schema: Schema.Decoder<Decoded>,
  frame: Uint8Array,
) => Effect.Effect<Decoded, string> = (schema, frame) =>
  Schema.decodeUnknownEffect(Schema.fromJsonString(schema))(new TextDecoder().decode(frame)).pipe(
    Effect.mapError(described),
  );

const read: <Decoded>(
  schema: Schema.Decoder<Decoded>,
  stream: Pick<Stream, "recv">,
) => Effect.Effect<Decoded | undefined, string> = (schema, stream) =>
  Effect.tryPromise({ catch: described, try: () => stream.recv() }).pipe(
    Effect.flatMap((frame) => {
      if (frame === undefined) {
        return Effect.succeed(frame);
      }
      return decode(schema, frame);
    }),
  );

const open: (connection: Connection) => Effect.Effect<Stream, string> = (connection) =>
  Effect.tryPromise({ catch: described, try: () => connection.openBi() });

const closing: (connection: Connection) => Effect.Effect<never, string> = (connection) =>
  Effect.promise(() => connection.closed()).pipe(
    Effect.flatMap((reason) => Effect.fail(`the connection closed: ${reason}`)),
  );

const close: (connection: Connection) => Effect.Effect<void> = (connection) =>
  Effect.tryPromise(() => connection.close()).pipe(Effect.ignore);

export { close, closing, described, open, read, write };
