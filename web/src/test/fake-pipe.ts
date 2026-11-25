import type { Connection, Pipe, Stream } from "#ui/pipe.ts";
import { Deferred, Effect, Option, Queue } from "effect";
import type { Cause } from "effect";

type Frames = Queue.Queue<Uint8Array, Cause.Done>;

type Frame = Parameters<Stream["send"]>[0];

interface HostStream {
  readonly send: (frame: Frame) => Promise<void>;
  readonly finish: () => void;
  readonly recv: () => Promise<Uint8Array | undefined>;
}

const next = (queue: Frames): Promise<Uint8Array | undefined> =>
  Effect.runPromise(Queue.take(queue).pipe(Effect.option, Effect.map(Option.getOrUndefined)));

const send = (queue: Frames, frame: Frame): void => {
  Queue.offerUnsafe(queue, Uint8Array.from(frame));
};

const end = (queue: Frames): void => {
  Queue.endUnsafe(queue);
};

const appStream = (up: Frames, down: Frames): Stream => ({
  [Symbol.dispose]: (): void => undefined,
  finish: (): Promise<void> =>
    Effect.runPromise(
      Effect.sync(() => {
        end(up);
      }),
    ),
  free: (): void => undefined,
  recv: (): Promise<Uint8Array | undefined> => next(down),
  send: (frame: Frame): Promise<void> =>
    Effect.runPromise(
      Effect.sync(() => {
        send(up, frame);
      }),
    ),
});

const hostStream = (up: Frames, down: Frames): HostStream => ({
  finish: (): void => {
    end(down);
  },
  recv: (): Promise<Uint8Array | undefined> => next(up),
  send: (frame: Frame): Promise<void> =>
    Effect.runPromise(
      Effect.sync(() => {
        send(down, frame);
      }),
    ),
});

const closeWith = (closed: Deferred.Deferred<string>, reason: string): void => {
  Deferred.doneUnsafe(closed, Effect.succeed(reason));
};

const frames = (): Effect.Effect<Frames> => Queue.unbounded<Uint8Array, Cause.Done>();

const openBi = (opened: Queue.Queue<HostStream>): Promise<Stream> =>
  Effect.runPromise(
    Effect.gen(function* open() {
      const up = yield* frames();
      const down = yield* frames();
      yield* Queue.offer(opened, hostStream(up, down));
      return appStream(up, down);
    }),
  );

const fake = (opened: Queue.Queue<HostStream>, closed: Deferred.Deferred<string>): FakeHost => {
  const connection: Connection = {
    [Symbol.dispose]: (): void => undefined,
    close: (): Promise<void> =>
      Effect.runPromise(
        Effect.sync(() => {
          closeWith(closed, "closed");
        }),
      ),
    closed: (): Promise<string> => Effect.runPromise(Deferred.await(closed)),
    free: (): void => undefined,
    openBi: (): Promise<Stream> => openBi(opened),
  };
  return {
    accept: (): Promise<HostStream> => Effect.runPromise(Queue.take(opened)),
    close: (reason: string): void => {
      closeWith(closed, reason);
    },
    pipe: { connect: (): Promise<Connection> => Promise.resolve(connection) },
  };
};

export interface FakeHost {
  readonly pipe: Pipe;
  readonly accept: () => Promise<HostStream>;
  readonly close: (reason: string) => void;
}

export const fakeHost = (): Promise<FakeHost> =>
  Effect.runPromise(
    Queue.unbounded<HostStream>().pipe(
      Effect.map((opened) => fake(opened, Deferred.makeUnsafe<string>())),
    ),
  );
