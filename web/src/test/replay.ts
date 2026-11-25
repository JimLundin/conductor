import { CommandResult, Feed, HelloReply, Open } from "@conductor/core/schemas/index";
import type { Connection, Pipe } from "#ui/pipe.ts";
import { Deferred, Effect, Ref, Stream } from "effect";
import type { Subscribe, Ticket, Welcome } from "@conductor/core/schemas/index";
import { Update, isDelta } from "@conductor/core/link";
import { read, write } from "#ui/link/frames.ts";
import type { FakeHost } from "#ui/test/fake-pipe.ts";
import type { HostLink } from "#ui/link/host.ts";
import type { LinkState } from "@conductor/core/link";
import type { RecordedFeed } from "#ui/test/recorded-feeds.ts";
import type { Scope } from "effect";
import { recordedFeed } from "#ui/test/recorded-feeds.ts";

type Entry = RecordedFeed[number];

type Sent = Extract<Open, { readonly Command: unknown }>["Command"];

interface Scenario {
  readonly welcome: (connection: number) => Effect.Effect<Welcome>;
  readonly feed: (connection: number) => ReadonlyArray<Feed>;
  readonly results: (connection: number) => ReadonlyArray<CommandResult>;
}

const helloOf = (entry: Entry): ReadonlyArray<HelloReply> => {
  if (entry.stream === "hello") {
    return [entry.frame];
  }
  return [];
};

const feedOf = (entry: Entry): ReadonlyArray<Feed> => {
  if (entry.stream === "feed") {
    return [entry.frame];
  }
  return [];
};

const resultOf = (entry: Entry): ReadonlyArray<CommandResult> => {
  if (entry.stream === "command") {
    return [entry.frame];
  }
  return [];
};

const welcomeOf = (reply: HelloReply): ReadonlyArray<Welcome> => {
  if ("Welcome" in reply) {
    return [reply.Welcome];
  }
  return [];
};

const scenarioOf = (entries: ReadonlyArray<Entry>): Scenario => {
  const on = (connection: number): ReadonlyArray<Entry> =>
    entries.filter((entry) => entry.connection === connection);
  return {
    feed: (connection: number): ReadonlyArray<Feed> =>
      on(connection).flatMap((entry) => feedOf(entry)),
    results: (connection: number): ReadonlyArray<CommandResult> =>
      on(connection).flatMap((entry) => resultOf(entry)),
    welcome: (connection: number): Effect.Effect<Welcome> =>
      Effect.fromNullishOr(
        on(connection)
          .flatMap((entry) => helloOf(entry))
          .flatMap((reply) => welcomeOf(reply))[0],
      ).pipe(Effect.orDie),
  };
};

const scenario = (name: string): Scenario => scenarioOf(recordedFeed(name));

const isDeltaFrame = (frame: Feed): boolean => "Event" in frame && isDelta(frame.Event);

const stored = (frames: ReadonlyArray<Feed>): ReadonlyArray<Feed> =>
  frames.filter((frame) => "Event" in frame && !isDeltaFrame(frame));

const events = (updates: ReadonlyArray<Update>): ReadonlyArray<number> =>
  updates.flatMap((update) => {
    if (Update.$is("Feed")(update) && "Event" in update.frame && !isDeltaFrame(update.frame)) {
      return [update.frame.Event.seq];
    }
    return [];
  });

const states = (updates: ReadonlyArray<Update>): ReadonlyArray<LinkState> =>
  updates.flatMap((update) => {
    if (Update.$is("Status")(update)) {
      return [update.state];
    }
    return [];
  });

const deltas = (updates: ReadonlyArray<Update>): ReadonlyArray<Feed> =>
  updates.flatMap((update) => {
    if (Update.$is("Feed")(update) && isDeltaFrame(update.frame)) {
      return [update.frame];
    }
    return [];
  });

const endpointBytes = 32;
const tokenBytes = 16;

const ticket: Ticket = {
  host: Array.from({ length: endpointBytes }, () => 1),
  relay: "https://relay.example/",
  token: Array.from({ length: tokenBytes }, () => 1),
} as const;

const client = "test-client";

type HostStream = Awaited<ReturnType<FakeHost["accept"]>>;

interface Served {
  readonly subscribes: Ref.Ref<ReadonlyArray<Subscribe>>;
  readonly commands: Ref.Ref<ReadonlyArray<Sent>>;
}

interface Replay {
  readonly reply: HelloReply;
  readonly feed: ReadonlyArray<Feed>;
  readonly results: ReadonlyArray<CommandResult>;
  readonly answers?: ReadonlyArray<ReadonlyArray<Feed>>;
}

interface Script {
  readonly results: Ref.Ref<ReadonlyArray<CommandResult>>;
  readonly answers: Ref.Ref<ReadonlyArray<ReadonlyArray<Feed>>>;
  readonly subscribed: Deferred.Deferred<HostStream>;
}

const next = <Value>(left: Ref.Ref<ReadonlyArray<Value>>): Effect.Effect<ReadonlyArray<Value>> =>
  Ref.modify(left, (all) => [all.slice(0, 1), all.slice(1)]);

const answered = (script: Script): Effect.Effect<void> =>
  next(script.answers).pipe(
    Effect.flatMap((frames) =>
      Effect.forEach(frames.flat(), (frame) =>
        Deferred.await(script.subscribed).pipe(Effect.flatMap((feed) => write(Feed, feed, frame))),
      ),
    ),
    Effect.orDie,
  );

const answer =
  (replay: Replay, served: Served, script: Script) =>
  (stream: HostStream): Effect.Effect<void> =>
    read(Open, stream).pipe(
      Effect.flatMap(Effect.fromNullishOr),
      Effect.flatMap((open) => {
        if ("Hello" in open) {
          return write(HelloReply, stream, replay.reply);
        }
        if ("Subscribe" in open) {
          return Ref.update(served.subscribes, (all) => [...all, open.Subscribe]).pipe(
            Effect.andThen(Effect.forEach(replay.feed, (frame) => write(Feed, stream, frame))),
            Effect.andThen(Deferred.succeed(script.subscribed, stream)),
          );
        }
        if ("Command" in open) {
          return Ref.update(served.commands, (all) => [...all, open.Command]).pipe(
            Effect.andThen(next(script.results)),
            Effect.flatMap((results) =>
              Effect.forEach(results, (result) => write(CommandResult, stream, result)),
            ),
            Effect.andThen(
              Effect.sync(() => {
                stream.finish();
              }),
            ),
            Effect.andThen(answered(script)),
          );
        }
        return Effect.void;
      }),
      Effect.orDie,
    );

const serve = (host: FakeHost, replay: Replay): Effect.Effect<Served, never, Scope.Scope> =>
  Effect.gen(function* serving() {
    const served: Served = {
      commands: yield* Ref.make<ReadonlyArray<Sent>>([]),
      subscribes: yield* Ref.make<ReadonlyArray<Subscribe>>([]),
    };
    const script: Script = {
      answers: yield* Ref.make(replay.answers ?? []),
      results: yield* Ref.make(replay.results),
      subscribed: yield* Deferred.make<HostStream>(),
    };
    yield* Effect.promise(() => host.accept()).pipe(
      Effect.flatMap((stream) => Effect.forkScoped(answer(replay, served, script)(stream))),
      Effect.forever,
      Effect.forkScoped,
    );
    return served;
  });

const reach = (host: FakeHost): Promise<Connection> =>
  host.pipe.connect(new Uint8Array(), "", new Uint8Array());

const stalled = (): Promise<Connection> => Effect.runPromise(Effect.never);

const dialing = (dials: ReadonlyArray<() => Promise<Connection>>): Pipe => {
  const left = [...dials];
  return {
    connect: (): Promise<Connection> => (left.shift() ?? stalled)(),
  };
};

const hosts = (fakes: ReadonlyArray<FakeHost>): Pipe =>
  dialing(fakes.map((fake) => (): Promise<Connection> => reach(fake)));

const until = (
  linked: HostLink,
  isLast: (update: Update) => boolean,
): Effect.Effect<ReadonlyArray<Update>> =>
  linked.updates.pipe(
    Stream.takeUntil(isLast),
    Stream.runCollect,
    Effect.map((updates) => [...updates]),
  );

const throughFeed = (linked: HostLink, count: number): Effect.Effect<ReadonlyArray<Update>> =>
  Effect.suspend(() => {
    let seen = 0;
    return until(linked, (update) => {
      if (Update.$is("Feed")(update)) {
        seen += 1;
      }
      return seen === count;
    });
  });

const inState =
  (state: LinkState) =>
  (update: Update): boolean =>
    Update.$is("Status")(update) && update.state === state;

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
  serve,
  states,
  stored,
  throughFeed,
  ticket,
  until,
};
