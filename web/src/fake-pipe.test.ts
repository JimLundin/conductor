import { describe, expect, it } from "@effect/vitest";
import type { Connection } from "#ui/pipe.ts";
import { Effect } from "effect";
import type { FakeHost } from "#ui/test/fake-pipe.ts";
import { fakeHost } from "#ui/test/fake-pipe.ts";

const bytes = (text: string): Uint8Array => new TextEncoder().encode(text);
const dial = (host: FakeHost): Effect.Effect<Connection, unknown> =>
  Effect.tryPromise(() =>
    host.pipe.connect(new Uint8Array(32), "https://relay.example/", bytes("conductor/0")),
  );

describe("app frames", () => {
  it.effect("delivers the app's frames to the host in order, then the end", () =>
    Effect.gen(function* verify1() {
      const host = yield* Effect.tryPromise(fakeHost);
      const connection = yield* dial(host);
      const stream = yield* Effect.tryPromise(() => connection.openBi());
      yield* Effect.tryPromise(() => stream.send(bytes("one")));
      yield* Effect.tryPromise(() => stream.send(bytes("")));
      yield* Effect.tryPromise(() => stream.finish());
      const accepted = yield* Effect.tryPromise(() => host.accept());
      expect(yield* Effect.tryPromise(() => accepted.recv())).toStrictEqual(bytes("one"));
      expect(yield* Effect.tryPromise(() => accepted.recv())).toStrictEqual(bytes(""));
      expect(yield* Effect.tryPromise(() => accepted.recv())).toBeUndefined();
    }),
  );
});

describe("host frames", () => {
  it.effect("delivers the host's frames to the app, then the end", () =>
    Effect.gen(function* verify2() {
      const host = yield* Effect.tryPromise(fakeHost);
      const connection = yield* dial(host);
      const stream = yield* Effect.tryPromise(() => connection.openBi());
      const accepted = yield* Effect.tryPromise(() => host.accept());
      yield* Effect.tryPromise(() => accepted.send(bytes("welcome")));
      accepted.finish();
      expect(yield* Effect.tryPromise(() => stream.recv())).toStrictEqual(bytes("welcome"));
      expect(yield* Effect.tryPromise(() => stream.recv())).toBeUndefined();
    }),
  );
});

describe("independent streams", () => {
  it.effect("keeps each stream separate", () =>
    Effect.gen(function* verify3() {
      const host = yield* Effect.tryPromise(fakeHost);
      const connection = yield* dial(host);
      const first = yield* Effect.tryPromise(() => connection.openBi());
      const second = yield* Effect.tryPromise(() => connection.openBi());
      yield* Effect.tryPromise(() => second.send(bytes("second")));
      yield* Effect.tryPromise(() => first.send(bytes("first")));
      const firstAccepted = yield* Effect.tryPromise(() => host.accept());
      const secondAccepted = yield* Effect.tryPromise(() => host.accept());
      expect(yield* Effect.tryPromise(() => firstAccepted.recv())).toStrictEqual(bytes("first"));
      expect(yield* Effect.tryPromise(() => secondAccepted.recv())).toStrictEqual(bytes("second"));
    }),
  );
});

describe("host close", () => {
  it.effect("resolves closed with the host's reason", () =>
    Effect.gen(function* verify4() {
      const host = yield* Effect.tryPromise(fakeHost);
      const connection = yield* dial(host);
      host.close("bye");
      expect(yield* Effect.tryPromise(() => connection.closed())).toBe("bye");
    }),
  );
});

describe("app close", () => {
  it.effect("resolves closed when the app closes", () =>
    Effect.gen(function* verify5() {
      const host = yield* Effect.tryPromise(fakeHost);
      const connection = yield* dial(host);
      yield* Effect.tryPromise(() => connection.close());
      expect(yield* Effect.tryPromise(() => connection.closed())).toBe("closed");
    }),
  );
});
