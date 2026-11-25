import { Effect, Random } from "effect";
import {
  client,
  dialing,
  inState,
  reach,
  scenario,
  serve,
  stored,
  ticket,
  until,
} from "#ui/test/replay.ts";
import { describe, expect, it } from "@effect/vitest";
import type { Connection } from "#ui/pipe.ts";
import type { FakeHost } from "#ui/test/fake-pipe.ts";
import type { HostLink } from "#ui/link/host.ts";
import type { Scope } from "effect";
import { TestClock } from "effect/testing";
import { fakeHost } from "#ui/test/fake-pipe.ts";
import { link } from "#ui/link.ts";

const turn = (): ReturnType<typeof scenario> => scenario("a_claude_turn_streams_asks_and_finishes");

interface DialTrace {
  readonly linked: HostLink;
  readonly times: () => ReadonlyArray<number>;
}

const replaying = (): Effect.Effect<FakeHost, unknown, Scope.Scope> =>
  Effect.gen(function* replay() {
    const host = yield* Effect.tryPromise(fakeHost);
    yield* serve(host, {
      feed: stored(turn().feed(1)),
      reply: { Welcome: yield* turn().welcome(1) },
      results: [],
    });
    return host;
  });

const traced = (host: FakeHost): Effect.Effect<DialTrace, never, Scope.Scope> =>
  Effect.gen(function* recording() {
    const clock = yield* TestClock.testClockWith(Effect.succeed);
    let at: ReadonlyArray<number> = [];
    const timed = (dial: () => Promise<Connection>) => (): Promise<Connection> => {
      at = [...at, clock.currentTimeMillisUnsafe()];
      return dial();
    };
    const offline = timed(() => Promise.reject(new Error("offline")));
    const linked = yield* link(
      dialing([
        ...Array.from({ length: 7 }, () => offline),
        timed(() => reach(host)),
        timed(() => Effect.runPromise(Effect.never)),
      ]),
      ticket,
      client,
    );
    return { linked, times: (): ReadonlyArray<number> => at };
  });

// Every jitter draws 0.75, which stretches a delay by 1.1 within its 0.8 to 1.2.
const drawing = {
  nextDoubleUnsafe: (): number => 0.75,
  nextIntUnsafe: (): number => 0,
} as const satisfies Random.Random;

describe("reconnect delays", () => {
  it.effect(
    "the link backs off from 250 ms, doubling to 10 s with jitter, and from 250 ms again after Live",
    () =>
      Effect.gen(function* verify6() {
        const dials = yield* Effect.scoped(
          Effect.gen(function* backingOff() {
            const host = yield* replaying();
            const { linked, times } = yield* traced(host);
            for (const delay of [275, 550, 1100, 2200, 4400, 8800, 11_000]) {
              yield* until(linked, inState("Disconnected"));
              yield* TestClock.adjust(delay);
            }
            yield* until(linked, inState("Live"));
            host.close("restarted");
            yield* until(linked, inState("Disconnected"));
            yield* TestClock.adjust(275);
            return times();
          }).pipe(Effect.provideService(Random.Random, drawing)),
        );
        expect(dials.slice(1).map((time, index) => time - (dials[index] ?? 0))).toStrictEqual([
          275, 550, 1100, 2200, 4400, 8800, 11_000, 275,
        ]);
      }),
  );
});
