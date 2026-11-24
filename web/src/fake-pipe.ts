// An in-memory pipe for tests, where the test plays the host. Its connections and streams are
// typed as web-iroh's, so a change to the pipe's declarations breaks the type-check here.
import { type Cause, Deferred, Effect, Queue } from "effect";
import type { Connection, Pipe, Stream } from "./pipe.ts";

// One direction of a stream: its frames in order, then its end.
type Frames = Queue.Queue<Uint8Array, Cause.Done>;

type Frame = Parameters<Stream["send"]>[0];

// The host's side of a stream the app opened.
interface HostStream {
  readonly send: (frame: Uint8Array) => void;
  readonly finish: () => void;
  // The app's next frame, or undefined once the app finished the stream.
  readonly recv: () => Promise<Uint8Array | undefined>;
}

const next = (queue: Frames): Promise<Uint8Array | undefined> =>
  Effect.runPromise(Queue.take(queue).pipe(Effect.orElseSucceed(() => undefined)));

const send = (queue: Frames, frame: Frame): void => {
  Queue.offerUnsafe(queue, Uint8Array.from(frame));
};

const end = (queue: Frames): void => {
  Queue.endUnsafe(queue);
};

const appStream = (up: Frames, down: Frames): Stream => ({
  free: (): void => undefined,
  [Symbol.dispose]: (): void => undefined,
  send: (frame: Frame): Promise<void> => Promise.resolve(send(up, frame)),
  recv: (): Promise<Uint8Array | undefined> => next(down),
  finish: (): Promise<void> => Promise.resolve(end(up)),
});

const hostStream = (up: Frames, down: Frames): HostStream => ({
  send: (frame: Uint8Array): void => send(down, frame),
  finish: (): void => end(down),
  recv: (): Promise<Uint8Array | undefined> => next(up),
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
    free: (): void => undefined,
    [Symbol.dispose]: (): void => undefined,
    openBi: (): Promise<Stream> => openBi(opened),
    closed: (): Promise<string> => Effect.runPromise(Deferred.await(closed)),
    close: (): Promise<void> => Promise.resolve(closeWith(closed, "closed")),
  };
  return {
    pipe: { connect: (): Promise<Connection> => Promise.resolve(connection) },
    accept: (): Promise<HostStream> => Effect.runPromise(Queue.take(opened)),
    close: (reason: string): void => closeWith(closed, reason),
  };
};

export interface FakeHost {
  readonly pipe: Pipe;
  // The host's side of the next stream the app opens.
  readonly accept: () => Promise<HostStream>;
  readonly close: (reason: string) => void;
}

export const fakeHost = (): Promise<FakeHost> =>
  Effect.runPromise(
    Queue.unbounded<HostStream>().pipe(
      Effect.map((opened) => fake(opened, Deferred.makeUnsafe<string>())),
    ),
  );
