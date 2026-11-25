import { Deferred, Effect } from "effect";
import {
  client,
  dialing,
  inState,
  reach,
  scenario,
  serve,
  states,
  ticket,
  until,
} from "#ui/test/replay.ts";
import { describe, expect, it } from "@effect/vitest";
import type { Connection } from "#ui/pipe.ts";
import { fakeHost } from "#ui/test/fake-pipe.ts";
import { link } from "#ui/link.ts";

const turn = () => scenario("a_claude_turn_streams_asks_and_finishes");

const stalled = (): Promise<Connection> => Effect.runPromise(Effect.never);

describe("stalled connections", () => {
  it.effect(
    "a stalled dial goes Live through a fresh dial beside it, and the link stops cleanly",
    () =>
      Effect.gen(function* verify1() {
        const updates = yield* Effect.scoped(
          Effect.gen(function* hedged() {
            const host = yield* Effect.tryPromise(fakeHost);
            yield* serve(host, {
              feed: turn().feed(0),
              reply: { Welcome: yield* turn().welcome(0) },
              results: [],
            });
            const linked = yield* link(
              dialing([stalled, (): Promise<Connection> => reach(host)]),
              ticket,
              client,
            );
            return yield* until(linked, inState("Live"));
          }),
        );
        expect(states(updates)).toStrictEqual([
          "Connecting",
          "Authenticating",
          "CatchingUp",
          "Live",
        ]);
      }),
  );
});

describe("late connections", () => {
  it.effect("a stalled dial that connects after the fresh one won is closed", () =>
    Effect.gen(function* verify2() {
      const closed = yield* Effect.scoped(
        Effect.gen(function* hedged() {
          const [host, late] = [
            yield* Effect.tryPromise(fakeHost),
            yield* Effect.tryPromise(fakeHost),
          ];
          const connected = yield* Deferred.make<Connection>();
          const slow = (): Promise<Connection> => Effect.runPromise(Deferred.await(connected));
          yield* serve(host, {
            feed: turn().feed(0),
            reply: { Welcome: yield* turn().welcome(0) },
            results: [],
          });
          const linked = yield* link(
            dialing([slow, (): Promise<Connection> => reach(host)]),
            ticket,
            client,
          );
          yield* until(linked, inState("Live"));
          const loser = yield* Effect.tryPromise(() => reach(late));
          yield* Deferred.succeed(connected, loser);
          return yield* Effect.tryPromise(() => loser.closed());
        }),
      );
      expect(closed).toBe("closed");
    }),
  );
});
