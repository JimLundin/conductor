// The recorded host feeds, and a fake host that replays one to a link over the fake pipe.
import { Effect, Ref, Schema, type Scope, Stream } from "effect";
import type { FakeHost } from "../fake-pipe.ts";
import { isDelta } from "../fold/host.ts";
import type { HostLink, LinkState, Update } from "./host.ts";
import type { Connection, Pipe } from "../pipe.ts";
import {
	type Command,
	CommandResult,
	Feed,
	HelloReply,
	Open,
	type Subscribe,
	type Ticket,
	type Welcome,
} from "../proto/index.ts";
import { type RecordedFeed, recordedFeed } from "../recorded-feeds.ts";

type Entry = RecordedFeed[number];

interface Scenario {
	readonly welcome: (connection: number) => Effect.Effect<Welcome>;
	readonly feed: (connection: number) => readonly Feed[];
	readonly results: (connection: number) => readonly CommandResult[];
}

const helloOf = (entry: Entry): readonly HelloReply[] => {
	if (entry.stream === "hello") {
		return [entry.frame];
	}
	return [];
};

const feedOf = (entry: Entry): readonly Feed[] => {
	if (entry.stream === "feed") {
		return [entry.frame];
	}
	return [];
};

const resultOf = (entry: Entry): readonly CommandResult[] => {
	if (entry.stream === "command") {
		return [entry.frame];
	}
	return [];
};

const welcomeOf = (reply: HelloReply): readonly Welcome[] => {
	if ("Welcome" in reply) {
		return [reply.Welcome];
	}
	return [];
};

const scenarioOf = (entries: readonly Entry[]): Scenario => {
	const on = (connection: number): readonly Entry[] =>
		entries.filter((entry) => entry.connection === connection);
	return {
		welcome: (connection: number): Effect.Effect<Welcome> =>
			Effect.fromNullishOr(
				on(connection).flatMap(helloOf).flatMap(welcomeOf)[0],
			).pipe(Effect.orDie),
		feed: (connection: number): readonly Feed[] =>
			on(connection).flatMap(feedOf),
		results: (connection: number): readonly CommandResult[] =>
			on(connection).flatMap(resultOf),
	};
};

// The recorded feed `name`, by its connections.
const scenario = (name: string): Scenario => scenarioOf(recordedFeed(name));

// The frames a host would replay: every stored event, and no live-only delta.
const isDeltaFrame = (frame: Feed): boolean =>
	"Event" in frame && isDelta(frame.Event);

const stored = (frames: readonly Feed[]): readonly Feed[] =>
	frames.filter((frame) => "Event" in frame && !isDeltaFrame(frame));

// The seq of each stored event in `updates`, in order.
const events = (updates: readonly Update[]): readonly number[] =>
	updates.flatMap((update) => {
		if (
			update._tag === "Feed" &&
			"Event" in update.frame &&
			!isDeltaFrame(update.frame)
		) {
			return [update.frame.Event.seq];
		}
		return [];
	});

// Each status's state in `updates`, in order.
const states = (updates: readonly Update[]): readonly LinkState[] =>
	updates.flatMap((update) => {
		if (update._tag === "Status") {
			return [update.state];
		}
		return [];
	});

// Each delta in `updates`, in order.
const deltas = (updates: readonly Update[]): readonly Feed[] =>
	updates.flatMap((update) => {
		if (update._tag === "Feed" && isDeltaFrame(update.frame)) {
			return [update.frame];
		}
		return [];
	});

// A ticket naming no real host, for the fake pipe.
const endpointBytes = 32;
const tokenBytes = 16;

// A ticket naming no real host, for the fake pipe.
const ticket: Ticket = {
	host: Array.from({ length: endpointBytes }, () => 1),
	relay: "https://relay.example/",
	token: Array.from({ length: tokenBytes }, () => 1),
} as const;

const client = "test-client";

type HostStream = Awaited<ReturnType<FakeHost["accept"]>>;

const encoder = new TextEncoder();
const decoder = new TextDecoder();

const sendFrame = <T, E>(
	schema: Schema.Codec<T, E>,
	stream: HostStream,
	frame: T,
): Effect.Effect<void> =>
	Schema.encodeEffect(schema)(frame).pipe(
		Effect.map((json) => stream.send(encoder.encode(JSON.stringify(json)))),
		Effect.orDie,
	);

const opened = (stream: HostStream): Effect.Effect<Open> =>
	Effect.promise(() => stream.recv()).pipe(
		Effect.flatMap((frame) =>
			Schema.decodeUnknownEffect(Schema.fromJsonString(Open))(
				decoder.decode(frame),
			),
		),
		Effect.orDie,
	);

// What the app asked the host for.
interface Served {
	readonly subscribes: Ref.Ref<readonly Subscribe[]>;
	readonly commands: Ref.Ref<readonly Command[]>;
}

interface Replay {
	readonly reply: HelloReply;
	readonly feed: readonly Feed[];
	readonly results: readonly CommandResult[];
}

const answer = (
	stream: HostStream,
	replay: Replay,
	served: Served,
	results: Ref.Ref<readonly CommandResult[]>,
): Effect.Effect<void> =>
	opened(stream).pipe(
		Effect.flatMap((open) => {
			if ("Hello" in open) {
				return sendFrame(HelloReply, stream, replay.reply);
			}
			if ("Subscribe" in open) {
				return Ref.update(served.subscribes, (all) => [
					...all,
					open.Subscribe,
				]).pipe(
					Effect.andThen(
						Effect.forEach(replay.feed, (frame) =>
							sendFrame(Feed, stream, frame),
						),
					),
				);
			}
			if ("Command" in open) {
				return Ref.update(served.commands, (all) => [
					...all,
					open.Command,
				]).pipe(
					Effect.andThen(
						Ref.modify(results, (left) => [left.slice(0, 1), left.slice(1)]),
					),
					Effect.flatMap((next) =>
						Effect.forEach(next, (result) =>
							sendFrame(CommandResult, stream, result),
						),
					),
					Effect.andThen(Effect.sync(() => stream.finish())),
				);
			}
			return Effect.void;
		}),
	);

// Plays one connection of a host: `reply` to the hello, `feed` on each subscription, and the
// next of `results` to each command.
const serve = (
	host: FakeHost,
	replay: Replay,
): Effect.Effect<Served, never, Scope.Scope> =>
	Effect.gen(function* serving() {
		const served: Served = {
			subscribes: yield* Ref.make<readonly Subscribe[]>([]),
			commands: yield* Ref.make<readonly Command[]>([]),
		};
		const results = yield* Ref.make(replay.results);
		yield* Effect.promise(() => host.accept()).pipe(
			Effect.flatMap((stream) =>
				Effect.forkScoped(answer(stream, replay, served, results)),
			),
			Effect.forever,
			Effect.forkScoped,
		);
		return served;
	});

const reach = (host: FakeHost): Promise<Connection> =>
	host.pipe.connect(new Uint8Array(), "", new Uint8Array());

// A pipe whose each connect is the next of `dials`, and then one that never connects.
const dialing = (dials: readonly (() => Promise<Connection>)[]): Pipe => {
	const left = [...dials];
	const stalled = (): Promise<Connection> => Effect.runPromise(Effect.never);
	return {
		connect: (): Promise<Connection> => (left.shift() ?? stalled)(),
	};
};

// A pipe whose each connect reaches the next of `hosts`.
const hosts = (fakes: readonly FakeHost[]): Pipe =>
	dialing(fakes.map((fake) => (): Promise<Connection> => reach(fake)));

// The link's updates up to and including the first that `isLast` picks.
const until = (
	linked: HostLink,
	isLast: (update: Update) => boolean,
): Effect.Effect<readonly Update[]> =>
	linked.updates.pipe(
		Stream.takeUntil(isLast),
		Stream.runCollect,
		Effect.map((updates) => Array.from(updates)),
	);

// The link's updates up to and including its `count`th feed frame.
const throughFeed = (
	linked: HostLink,
	count: number,
): Effect.Effect<readonly Update[]> =>
	Effect.suspend(() => {
		let seen = 0;
		return until(linked, (update) => {
			if (update._tag === "Feed") {
				seen += 1;
			}
			return seen === count;
		});
	});

const inState =
	(state: LinkState) =>
	(update: Update): boolean =>
		update._tag === "Status" && update.state === state;

const scoped = <A, E>(effect: Effect.Effect<A, E, Scope.Scope>): Promise<A> =>
	Effect.runPromise(Effect.scoped(effect));

export {
	client,
	deltas,
	dialing,
	events,
	hosts,
	inState,
	isDeltaFrame,
	reach,
	scenario,
	scoped,
	serve,
	states,
	stored,
	throughFeed,
	ticket,
	until,
};
