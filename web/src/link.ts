// One client's link to one host over the pipe (architecture §7). Each paired host has its own,
// And all run at once.
import type { Access, Host, HostLink } from "#ui/link/host.ts";
import type {
  ClientName,
  Command,
  CommandResult,
  Ticket,
  Welcome,
} from "@conductor/core/schemas/index";
import type { Connection, Pipe } from "#ui/pipe.ts";
import type { Cursor, Status } from "@conductor/core/link";
import { Duration, Effect, Option, Queue, Ref, Schedule, Stream, SubscriptionRef } from "effect";
import { HelloReply, Open } from "@conductor/core/schemas/index";
import { SendError, Update } from "@conductor/core/link";
import { adopt, subscribe } from "#ui/link/feed.ts";
import { closing, open, read, write } from "#ui/link/frames.ts";
import { emit, publish, show } from "#ui/link/host.ts";
import type { Scope } from "effect";
import { command } from "#ui/link/command.ts";
import { connect } from "#ui/link/dial.ts";

const helloWithin: Duration.Input = "10 seconds";
const backoffFloor = Duration.fromInputUnsafe("250 millis");
const backoffCeiling = Duration.fromInputUnsafe("10 seconds");
const backoffFactor = 2;
const firstFailure = 0;
const nextFailure = 1;

const hello: (
  host: Host,
  connection: Connection,
) => Effect.Effect<HelloReply | undefined, string> = (host, connection) =>
  Effect.gen(function* greeting() {
    const stream = yield* open(connection);
    yield* write(Open, stream, {
      Hello: { client: host.client, token: host.ticket.token },
    });
    return yield* read(HelloReply, stream);
  });

const welcomed: (reply: HelloReply | undefined) => Effect.Effect<Welcome, string> = (reply) => {
  if (reply === undefined) {
    return Effect.fail("the host closed the hello stream");
  }
  if ("Refused" in reply) {
    return Effect.fail(`the host refused us: ${reply.Refused.reason}`);
  }
  return Effect.succeed(reply.Welcome);
};

const greet: (host: Host, connection: Connection) => Effect.Effect<Welcome, string> = (
  host,
  connection,
) =>
  hello(host, connection).pipe(
    Effect.flatMap(welcomed),
    Effect.timeoutOrElse({
      duration: helloWithin,
      orElse: () => Effect.fail("the host did not complete the hello handshake within 10 seconds"),
    }),
  );

const greeted: (host: Host, connection: Connection) => Effect.Effect<never, string> = (
  host,
  connection,
) =>
  Effect.gen(function* authenticating() {
    yield* show(host, "Authenticating");
    const welcome = yield* greet(host, connection);
    yield* adopt(host, welcome.epoch);
    yield* emit(host, Update.Welcome({ welcome }));
    yield* show(host, "CatchingUp");
    return yield* subscribe(host, connection, welcome);
  });

// One connection, from its dial until it ends.
const session: (host: Host) => Effect.Effect<never, string> = (host) =>
  Effect.scoped(
    Effect.gen(function* connecting() {
      yield* SubscriptionRef.set(host.access, "connecting");
      yield* show(host, "Connecting");
      const connection = yield* connect(host.pipe, host.ticket);
      return yield* Effect.raceFirst(greeted(host, connection), closing(connection));
    }),
  );

const disconnected: (host: Host, error?: string) => Effect.Effect<void> = (host, error) =>
  SubscriptionRef.set(host.access, "down").pipe(
    Effect.andThen(publish(host, { error, state: "Disconnected" })),
  );

// Whether the connection that ended had gone live, which starts the backoff over.
interface Dropped {
  readonly live: boolean;
}

const dropped: (host: Host) => Effect.Effect<never, Dropped> = (host) =>
  session(host).pipe(
    Effect.matchEffect({
      onFailure: (error) =>
        Effect.gen(function* down() {
          const { state } = yield* Ref.get(host.status);
          yield* disconnected(host, error);
          return yield* Effect.fail({ live: state === "Live" });
        }),
      onSuccess: Effect.succeed,
    }),
  );

const delayAfter: (failures: number) => Duration.Duration = (failures) =>
  Duration.min(Duration.times(backoffFloor, backoffFactor ** failures), backoffCeiling);

// Exponential from the floor to the ceiling, jittered so many clients don't retry in step, and
// From the floor again after a connection that went live.
const backoff: Schedule.Schedule<Duration.Duration, Dropped> = Schedule.fromStep(
  Effect.sync(() => {
    let failures = firstFailure;
    return (
      _now: number,
      ended: Dropped,
    ): Effect.Effect<[Duration.Duration, Duration.Duration]> => {
      if (ended.live) {
        failures = firstFailure;
      }
      const delay = delayAfter(failures);
      failures += nextFailure;
      return Effect.succeed([delay, delay]);
    };
  }),
).pipe(Schedule.jittered);

const run: (host: Host, reconnects: Queue.Queue<"Reconnect">) => Effect.Effect<never> = (
  host,
  reconnects,
) => {
  const retrying = Effect.retry(dropped(host), backoff).pipe(Effect.orDie);
  const reconnecting = Queue.take(reconnects).pipe(Effect.andThen(disconnected(host)));
  return Effect.forever(Effect.raceFirst(retrying, reconnecting));
};

const sendOn: (
  access: Option.Option<Access>,
  body: Command,
) => Effect.Effect<CommandResult, SendError> = (access, body) =>
  Option.filter(access, (value): value is Connection => typeof value !== "string").pipe(
    Option.match({
      onNone: () => Effect.fail(SendError.NotConnected()),
      onSome: (connection) => command(connection, body),
    }),
  );

const send =
  (host: Host) =>
  (body: Command): Effect.Effect<CommandResult, SendError> =>
    SubscriptionRef.changes(host.access).pipe(
      Stream.filter((access) => access !== "connecting"),
      Stream.runHead,
      Effect.flatMap((access) => sendOn(access, body)),
    );

// Starts a link to the host `ticket` names, greeting as `client`, from a fresh cursor. It runs
// Until its scope closes, which closes its connection.
const link: (
  pipe: Pipe,
  ticket: Ticket,
  client: ClientName,
) => Effect.Effect<HostLink, never, Scope.Scope> = (pipe, ticket, client) =>
  Effect.gen(function* started() {
    const host: Host = {
      access: yield* SubscriptionRef.make<Access>("connecting"),
      client,
      cursor: yield* Ref.make(Option.none<Cursor>()),
      pipe,
      status: yield* Ref.make<Status>({
        error: undefined,
        state: "Disconnected",
      }),
      ticket,
      updates: yield* Queue.unbounded<Update>(),
    };
    const reconnects = yield* Queue.unbounded<"Reconnect">();
    yield* Effect.forkScoped(run(host, reconnects));
    return {
      reconnect: Queue.offer(reconnects, "Reconnect").pipe(Effect.asVoid),
      send: send(host),
      updates: Stream.fromQueue(host.updates),
    };
  });

export { link };
