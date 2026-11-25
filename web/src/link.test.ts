import { Effect, Random, Ref } from "effect";
import {
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
} from "#ui/test/replay.ts";
import { describe, expect, it } from "@effect/vitest";
import type { Connection } from "#ui/pipe.ts";
import { TestClock } from "effect/testing";
import { Update } from "#ui/link/host.ts";
import type { Welcome } from "@conductor/core/schemas/index";
import { fakeHost } from "#ui/test/fake-pipe.ts";
import { link } from "#ui/link.ts";

const turn = () => scenario("a_claude_turn_streams_asks_and_finishes");

const walking = ["Connecting", "Authenticating", "CatchingUp", "Live"] as const;

// Every jitter draws 0.75, which stretches a delay by 1.1 within its 0.8 to 1.2.
const drawing = {
  nextDoubleUnsafe: (): number => 0.75,
  nextIntUnsafe: (): number => 0,
} as const satisfies Random.Random;

describe("initial connection", () => {
  it.effect("a host walks from a fresh cursor to Live, catching up to the welcome's head", () =>
    Effect.gen(function* verify1() {
      const { updates, subscribes, welcome } = yield* Effect.scoped(
        Effect.gen(function* walk() {
          const host = yield* Effect.tryPromise(fakeHost);
          const welcomed = yield* turn().welcome(1);
          const feed = turn().feed(1);
          const served = yield* serve(host, {
            feed,
            reply: { Welcome: welcomed },
            results: [],
          });
          const linked = yield* link(host.pipe, ticket, client);
          const seen = yield* throughFeed(linked, feed.length);
          return {
            subscribes: yield* Ref.get(served.subscribes),
            updates: seen,
            welcome: welcomed,
          };
        }),
      );
      expect(states(updates)).toStrictEqual(walking);
      expect(updates.slice(2, 4)).toStrictEqual([
        Update.Reset({ epoch: welcome.epoch }),
        Update.Welcome({ welcome }),
      ]);
      expect(subscribes).toStrictEqual([{ after_seq: 0, epoch: welcome.epoch }]);
      expect(events(updates.slice(0, updates.findIndex(inState("Live"))))).toStrictEqual([
        1, 2, 3, 4, 5, 6, 7, 8,
      ]);
      expect(updates.filter(Update.$is("Feed")).map((update) => update.frame)).toStrictEqual(
        turn().feed(1),
      );
    }),
  );
});

describe("dropped connection", () => {
  it.effect(
    "after the connection drops, the link resubscribes after the last applied seq, with no event duplicated or missed",
    () =>
      Effect.gen(function* verify2() {
        const { updates, subscribes, sent } = yield* Effect.scoped(
          Effect.gen(function* drop() {
            const [first, second] = [
              yield* Effect.tryPromise(fakeHost),
              yield* Effect.tryPromise(fakeHost),
            ];
            const reply = { Welcome: yield* turn().welcome(0) };
            const feed = turn().feed(0);
            const dropAt = feed.findIndex(isDeltaFrame) + 2;
            yield* serve(first, { feed: feed.slice(0, dropAt), reply, results: [] });
            const again = yield* serve(second, {
              feed: stored(feed),
              reply,
              results: [],
            });
            const linked = yield* link(hosts([first, second]), ticket, client);
            const before = yield* throughFeed(linked, dropAt);
            first.close("the network went away");
            const after = yield* until(linked, (update) => events([update]).includes(13));
            return {
              sent: feed.slice(0, dropAt),
              subscribes: yield* Ref.get(again.subscribes),
              updates: [...before, ...after],
            };
          }),
        );
        expect(subscribes).toStrictEqual([{ after_seq: 10, epoch: "1" }]);
        expect(events(updates)).toStrictEqual([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13]);
        expect(deltas(updates)).toStrictEqual(sent.filter(isDeltaFrame));
        expect(updates.filter(Update.$is("Reset"))).toHaveLength(1);
        expect(updates.find(inState("Disconnected"))).toStrictEqual(
          Update.Status({
            error: "the connection closed: the network went away",
            state: "Disconnected",
          }),
        );
      }),
  );
});

describe("changed epoch", () => {
  it.effect("on an epoch change, the link resets the host's history before the replay", () =>
    Effect.gen(function* verify3() {
      const { updates, subscribes, reborn } = yield* Effect.scoped(
        Effect.gen(function* rebirth() {
          const [first, second] = [
            yield* Effect.tryPromise(fakeHost),
            yield* Effect.tryPromise(fakeHost),
          ];
          const welcome = yield* turn().welcome(1);
          const restarted: Welcome = { ...welcome, epoch: `${welcome.epoch}0` };
          const feed = stored(turn().feed(1));
          yield* serve(first, { feed, reply: { Welcome: welcome }, results: [] });
          const again = yield* serve(second, {
            feed,
            reply: { Welcome: restarted },
            results: [],
          });
          const linked = yield* link(hosts([first, second]), ticket, client);
          yield* until(linked, inState("Live"));
          first.close("restarted");
          const seen = yield* until(linked, inState("Live"));
          return {
            reborn: restarted,
            subscribes: yield* Ref.get(again.subscribes),
            updates: seen,
          };
        }),
      );
      const reset = updates.findIndex(Update.$is("Reset"));
      expect(subscribes).toStrictEqual([{ after_seq: 0, epoch: reborn.epoch }]);
      expect(updates.slice(reset, reset + 2)).toStrictEqual([
        Update.Reset({ epoch: reborn.epoch }),
        Update.Welcome({ welcome: reborn }),
      ]);
      expect(events(updates.slice(0, reset))).toStrictEqual([]);
      expect(events(updates.slice(reset))).toStrictEqual([1, 2, 3, 4, 5, 6, 7, 8]);
    }),
  );
});

describe("requested reconnect", () => {
  it.effect(
    "a reconnect drops the live connection and connects again at once, after the last applied seq",
    () =>
      Effect.gen(function* verify4() {
        const { updates, subscribes, closed } = yield* Effect.scoped(
          Effect.gen(function* again() {
            const [first, second] = [
              yield* Effect.tryPromise(fakeHost),
              yield* Effect.tryPromise(fakeHost),
            ];
            const replay = {
              feed: stored(turn().feed(1)),
              reply: { Welcome: yield* turn().welcome(1) },
              results: [],
            };
            yield* serve(first, replay);
            const next = yield* serve(second, replay);
            const linked = yield* link(hosts([first, second]), ticket, client);
            yield* until(linked, inState("Live"));
            yield* linked.reconnect;
            const seen = yield* until(linked, inState("Live"));
            const dropped = yield* Effect.tryPromise(() => reach(first));
            return {
              closed: yield* Effect.tryPromise(() => dropped.closed()),
              subscribes: yield* Ref.get(next.subscribes),
              updates: seen,
            };
          }),
        );
        expect(states(updates)).toStrictEqual(["Disconnected", ...walking]);
        expect(updates.filter(Update.$is("Status")).map((update) => update.error)).toStrictEqual([
          undefined,
          undefined,
          undefined,
          undefined,
          undefined,
        ]);
        expect(subscribes).toStrictEqual([{ after_seq: 8, epoch: "1" }]);
        expect(events(updates)).toStrictEqual([]);
        expect(closed).toBe("closed");
      }),
  );
});

describe("refused connection", () => {
  it.effect("a refused hello drops the connection with the host's reason", () =>
    Effect.gen(function* verify5() {
      const updates = yield* Effect.scoped(
        Effect.gen(function* refused() {
          const host = yield* Effect.tryPromise(fakeHost);
          yield* serve(host, {
            feed: [],
            reply: { Refused: { reason: "unknown token" } },
            results: [],
          });
          const linked = yield* link(host.pipe, ticket, client);
          return yield* until(linked, inState("Disconnected"));
        }),
      );
      expect(updates.at(-1)).toStrictEqual(
        Update.Status({
          error: "the host refused us: unknown token",
          state: "Disconnected",
        }),
      );
    }),
  );
});

describe("reconnect delays", () => {
  it.effect(
    "the link backs off from 250 ms, doubling to 10 s with jitter, and from 250 ms again after Live",
    () =>
      Effect.gen(function* verify6() {
        const dials = yield* Effect.scoped(
          Effect.gen(function* backingOff() {
            const host = yield* Effect.tryPromise(fakeHost);
            yield* serve(host, {
              feed: stored(turn().feed(1)),
              reply: { Welcome: yield* turn().welcome(1) },
              results: [],
            });
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
            for (const delay of [275, 550, 1100, 2200, 4400, 8800, 11_000]) {
              yield* until(linked, inState("Disconnected"));
              yield* TestClock.adjust(delay);
            }
            yield* until(linked, inState("Live"));
            host.close("restarted");
            yield* until(linked, inState("Disconnected"));
            yield* TestClock.adjust(275);
            return at;
          }).pipe(Effect.provide(TestClock.layer()), Effect.provideService(Random.Random, drawing)),
        );
        expect(dials.slice(1).map((time, index) => time - (dials[index] ?? 0))).toStrictEqual([
          275, 550, 1100, 2200, 4400, 8800, 11_000, 275,
        ]);
      }),
  );
});
