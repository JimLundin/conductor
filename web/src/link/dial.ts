// Dials a host through its relay, and once more beside a dial that stalls.
import { type Duration, Effect, Exit, Option, Ref, type Scope } from "effect";
import type { Connection, Pipe } from "../pipe.ts";
import { Alpn, type Ticket } from "../proto/index.ts";
import { close, described } from "./frames.ts";

// A relay connection can stall for seconds while a fresh one is up in well under a second.
const hedgeAfter: Duration.Input = "1500 millis";

const alpn = (): Uint8Array => new TextEncoder().encode(Alpn.literal);

type Dials = Ref.Ref<readonly Promise<Connection>[]>;

const start = (
	pipe: Pipe,
	ticket: Ticket,
	dials: Dials,
): Effect.Effect<Promise<Connection>> =>
	Effect.sync(() =>
		pipe.connect(Uint8Array.from(ticket.host), ticket.relay, alpn()),
	).pipe(Effect.tap((dialed) => Ref.update(dials, (all) => [...all, dialed])));

const settle = (
	dialed: Promise<Connection>,
): Effect.Effect<Connection, string> =>
	Effect.tryPromise({
		try: () => dialed,
		catch: (error: unknown): string => `connect failed: ${described(error)}`,
	});

const closeUnless =
	(winner: Connection | undefined) =>
	(connection: Connection): Effect.Effect<void> => {
		if (connection === winner) {
			return Effect.void;
		}
		return close(connection);
	};

// Closes each dial's connection but the winner's, once it connects.
const discard = (
	dials: Dials,
	winner: Connection | undefined,
): Effect.Effect<void> =>
	Ref.get(dials).pipe(
		Effect.flatMap((all) =>
			Effect.forEach(
				all,
				(dialed) =>
					settle(dialed).pipe(
						Effect.flatMap(closeUnless(winner)),
						Effect.ignore,
					),
				{ concurrency: "unbounded", discard: true },
			),
		),
		Effect.forkDetach,
		Effect.asVoid,
	);

// The first dial, or the first of it and a fresh one if it hasn't connected by `hedgeAfter`.
const race = (
	pipe: Pipe,
	ticket: Ticket,
	dials: Dials,
): Effect.Effect<Connection, string> =>
	Effect.gen(function* hedged() {
		const base = yield* start(pipe, ticket, dials);
		const first = yield* settle(base).pipe(Effect.timeoutOption(hedgeAfter));
		if (Option.isSome(first)) {
			return first.value;
		}
		const fresh = yield* start(pipe, ticket, dials);
		return yield* Effect.race(settle(base), settle(fresh));
	});

const winnerOf = (
	exit: Exit.Exit<Connection, string>,
): Connection | undefined =>
	Exit.match(exit, {
		onSuccess: (connection: Connection): Connection | undefined => connection,
		onFailure: (): Connection | undefined => undefined,
	});

// A connection to the host that the scope closes.
const connect = (
	pipe: Pipe,
	ticket: Ticket,
): Effect.Effect<Connection, string, Scope.Scope> =>
	Ref.make<readonly Promise<Connection>[]>([]).pipe(
		Effect.flatMap((dials) =>
			Effect.uninterruptibleMask((restore) =>
				restore(race(pipe, ticket, dials)).pipe(
					Effect.onExit((exit) => discard(dials, winnerOf(exit))),
					Effect.tap((connection) =>
						Effect.addFinalizer(() => close(connection)),
					),
				),
			),
		),
	);

export { connect };
