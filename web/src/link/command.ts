// A command on its own stream, with its result for the caller.
import { Data, Effect, Random } from "effect";
import type { Connection } from "../pipe.ts";
import {
	type CommandBody,
	type CommandId,
	CommandResult,
} from "../proto/index.ts";
import { closing, described, open, read, write } from "./frames.ts";

type SendError = Data.TaggedEnum<{
	readonly NotConnected: Record<never, never>;
	readonly NoResult: Record<never, never>;
	readonly Wire: { readonly reason: string };
}>;

const SendError = Data.taggedEnum<SendError>();

const radix = 36;
const digits = 32;
// A ULID is 128 bits in 26 digits, so its first digit carries only 3 of them.
const firstDigits = 8;
const restLength = 25;

// Crockford's base 32: the base 36 digits without i, l, o and u.
const crockford = Array.from({ length: radix }, (_, value) =>
	value.toString(radix),
)
	.filter((character) => !"ilou".includes(character))
	.join("");

const digit = (below: number): Effect.Effect<string> =>
	Random.nextIntBetween(0, below, { halfOpen: true }).pipe(
		Effect.map((value) => crockford.charAt(value)),
	);

// A fleet-unique id: 128 random bits as a 26-digit lower-case ULID.
const commandId: Effect.Effect<CommandId> = Effect.all([
	digit(firstDigits),
	...Array.from({ length: restLength }, () => digit(digits)),
]).pipe(Effect.map((all) => all.join("")));

const answer = (
	connection: Connection,
	body: CommandBody,
): Effect.Effect<CommandResult | undefined, string> =>
	Effect.gen(function* rpc() {
		const id = yield* commandId;
		const stream = yield* open(connection);
		yield* write(stream, { Command: { id, body } });
		yield* Effect.tryPromise({ try: () => stream.finish(), catch: described });
		return yield* read(CommandResult, stream);
	});

const resulted = (
	result: CommandResult | undefined,
): Effect.Effect<CommandResult, SendError> => {
	if (result === undefined) {
		return Effect.fail(SendError.NoResult());
	}
	return Effect.succeed(result);
};

const command = (
	connection: Connection,
	body: CommandBody,
): Effect.Effect<CommandResult, SendError> =>
	Effect.raceFirst(answer(connection, body), closing(connection)).pipe(
		Effect.mapError((reason) => SendError.Wire({ reason })),
		Effect.flatMap(resulted),
	);

export { command, SendError };
