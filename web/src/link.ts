// One client's link to one host over the pipe (architecture §7). Each paired host has its own,
// and all run at once.
import {
	Duration,
	Effect,
	Option,
	Queue,
	Ref,
	Schedule,
	type Scope,
	Stream,
	SubscriptionRef,
} from "effect";
import { command, SendError } from "./link/command.ts";
import { connect } from "./link/dial.ts";
import { adopt, subscribe } from "./link/feed.ts";
import { closing, open, read, write } from "./link/frames.ts";
import {
	type Access,
	type Cursor,
	emit,
	type Host,
	type HostLink,
	publish,
	show,
	type Status,
	Update,
} from "./link/host.ts";
import type { Connection, Pipe } from "./pipe.ts";
import {
	type ClientName,
	type CommandBody,
	type CommandResult,
	HelloReply,
	type Ticket,
	type Welcome,
} from "./proto/index.ts";

const helloWithin: Duration.Input = "10 seconds";
const backoffFloor = Duration.fromInputUnsafe("250 millis");
const backoffCeiling = Duration.fromInputUnsafe("10 seconds");
const backoffFactor = 2;

const hello = (
	host: Host,
	connection: Connection,
): Effect.Effect<HelloReply | undefined, string> =>
	Effect.gen(function* greeting() {
		const stream = yield* open(connection);
		yield* write(stream, {
			Hello: { client: host.client, token: host.ticket.token },
		});
		return yield* read(HelloReply, stream);
	});

const welcomed = (
	reply: HelloReply | undefined,
): Effect.Effect<Welcome, string> => {
	if (reply === undefined) {
		return Effect.fail("the host closed the hello stream");
	}
	if ("Refused" in reply) {
		return Effect.fail(`the host refused us: ${reply.Refused.reason}`);
	}
	return Effect.succeed(reply.Welcome);
};

const greet = (
	host: Host,
	connection: Connection,
): Effect.Effect<Welcome, string> =>
	hello(host, connection).pipe(
		Effect.flatMap(welcomed),
		Effect.timeoutOrElse({
			duration: helloWithin,
			orElse: () =>
				Effect.fail(
					"the host did not complete the hello handshake within 10 seconds",
				),
		}),
	);

const greeted = (
	host: Host,
	connection: Connection,
): Effect.Effect<never, string> =>
	Effect.gen(function* authenticating() {
		yield* show(host, "Authenticating");
		const welcome = yield* greet(host, connection);
		yield* adopt(host, welcome.epoch);
		yield* emit(host, Update.Welcome({ welcome }));
		yield* show(host, "CatchingUp");
		return yield* subscribe(host, connection, welcome);
	});

// One connection, from its dial until it ends.
const session = (host: Host): Effect.Effect<never, string> =>
	Effect.scoped(
		Effect.gen(function* connecting() {
			yield* SubscriptionRef.set(host.access, "connecting");
			yield* show(host, "Connecting");
			const connection = yield* connect(host.pipe, host.ticket);
			return yield* Effect.raceFirst(
				greeted(host, connection),
				closing(connection),
			);
		}),
	);

const disconnected = (
	host: Host,
	error: string | undefined,
): Effect.Effect<void> =>
	SubscriptionRef.set(host.access, "down").pipe(
		Effect.andThen(publish(host, { state: "Disconnected", error })),
	);

// Whether the connection that ended had gone live, which starts the backoff over.
interface Dropped {
	readonly live: boolean;
}

const dropped = (host: Host): Effect.Effect<never, Dropped> =>
	session(host).pipe(
		Effect.catch((error) =>
			Effect.gen(function* down() {
				const { state } = yield* Ref.get(host.status);
				yield* disconnected(host, error);
				return yield* Effect.fail({ live: state === "Live" });
			}),
		),
	);

const delayAfter = (failures: number): Duration.Duration =>
	Duration.min(
		Duration.times(backoffFloor, backoffFactor ** failures),
		backoffCeiling,
	);

// Exponential from the floor to the ceiling, jittered so many clients don't retry in step, and
// from the floor again after a connection that went live.
const backoff: Schedule.Schedule<Duration.Duration, Dropped> =
	Schedule.fromStep(
		Effect.sync(() => {
			let failures = 0;
			return (
				_now: number,
				ended: Dropped,
			): Effect.Effect<[Duration.Duration, Duration.Duration]> => {
				if (ended.live) {
					failures = 0;
				}
				const delay = delayAfter(failures);
				failures += 1;
				return Effect.succeed([delay, delay]);
			};
		}),
	).pipe(Schedule.jittered);

const run = (host: Host, reconnects: Queue.Queue<void>): Effect.Effect<never> =>
	Effect.forever(
		Effect.raceFirst(
			Effect.retry(dropped(host), backoff).pipe(Effect.orDie),
			Queue.take(reconnects).pipe(
				Effect.andThen(disconnected(host, undefined)),
			),
		),
	);

const sendOn = (
	access: Option.Option<Access>,
	body: CommandBody,
): Effect.Effect<CommandResult, SendError> => {
	if (
		Option.isSome(access) &&
		access.value !== "down" &&
		access.value !== "connecting"
	) {
		return command(access.value, body);
	}
	return Effect.fail(SendError.NotConnected());
};

const send =
	(host: Host) =>
	(body: CommandBody): Effect.Effect<CommandResult, SendError> =>
		SubscriptionRef.changes(host.access).pipe(
			Stream.filter((access) => access !== "connecting"),
			Stream.runHead,
			Effect.flatMap((access) => sendOn(access, body)),
		);

// Starts a link to the host `ticket` names, greeting as `client`, from a fresh cursor. It runs
// until its scope closes, which closes its connection.
const link = (
	pipe: Pipe,
	ticket: Ticket,
	client: ClientName,
): Effect.Effect<HostLink, never, Scope.Scope> =>
	Effect.gen(function* started() {
		const host: Host = {
			pipe,
			ticket,
			client,
			updates: yield* Queue.unbounded<Update>(),
			status: yield* Ref.make<Status>({
				state: "Disconnected",
				error: undefined,
			}),
			cursor: yield* Ref.make(Option.none<Cursor>()),
			access: yield* SubscriptionRef.make<Access>("connecting"),
		};
		const reconnects = yield* Queue.unbounded<void>();
		yield* Effect.forkScoped(run(host, reconnects));
		return {
			updates: Stream.fromQueue(host.updates),
			send: send(host),
			reconnect: Queue.offer(reconnects, undefined).pipe(Effect.asVoid),
		};
	});

export { link };
