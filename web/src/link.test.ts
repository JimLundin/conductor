import { Effect, Ref } from "effect";
import { expect, test } from "vitest";
import { fakeHost } from "./fake-pipe.ts";
import { link } from "./link.ts";
import { Update } from "./link/host.ts";
import {
  client,
  deltas,
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
} from "./link/replay.ts";
import type { Welcome } from "./schemas/index.ts";

const turn = scenario("a_claude_turn_streams_asks_and_finishes");

const walking = ["Connecting", "Authenticating", "CatchingUp", "Live"] as const;

test("a host walks from a fresh cursor to Live, catching up to the welcome's head", async () => {
  const { updates, subscribes, welcome } = await scoped(
    Effect.gen(function* walk() {
      const host = yield* Effect.promise(fakeHost);
      const welcomed = yield* turn.welcome(1);
      const feed = turn.feed(1);
      const served = yield* serve(host, {
        reply: { Welcome: welcomed },
        feed,
        results: [],
      });
      const linked = yield* link(host.pipe, ticket, client);
      const seen = yield* throughFeed(linked, feed.length);
      return {
        updates: seen,
        subscribes: yield* Ref.get(served.subscribes),
        welcome: welcomed,
      };
    }),
  );
  expect(states(updates)).toEqual(walking);
  expect(updates.slice(2, 4)).toEqual([
    Update.Reset({ epoch: welcome.epoch }),
    Update.Welcome({ welcome }),
  ]);
  expect(subscribes).toEqual([{ epoch: welcome.epoch, after_seq: 0 }]);
  expect(events(updates.slice(0, updates.findIndex(inState("Live"))))).toEqual([
    1, 2, 3, 4, 5, 6, 7, 8,
  ]);
  expect(updates.filter(Update.$is("Feed")).map((update) => update.frame)).toEqual(turn.feed(1));
});

test("after the connection drops, the link resubscribes after the last applied seq, with no event duplicated or missed", async () => {
  const { updates, subscribes, sent } = await scoped(
    Effect.gen(function* drop() {
      const [first, second] = [yield* Effect.promise(fakeHost), yield* Effect.promise(fakeHost)];
      const reply = { Welcome: yield* turn.welcome(0) };
      const feed = turn.feed(0);
      const dropAt = feed.findIndex(isDeltaFrame) + 2;
      yield* serve(first, { reply, feed: feed.slice(0, dropAt), results: [] });
      const again = yield* serve(second, {
        reply,
        feed: stored(feed),
        results: [],
      });
      const linked = yield* link(hosts([first, second]), ticket, client);
      const before = yield* throughFeed(linked, dropAt);
      first.close("the network went away");
      const after = yield* until(linked, (update) => events([update]).includes(13));
      return {
        updates: [...before, ...after],
        subscribes: yield* Ref.get(again.subscribes),
        sent: feed.slice(0, dropAt),
      };
    }),
  );
  expect(subscribes).toEqual([{ epoch: "1", after_seq: 10 }]);
  expect(events(updates)).toEqual([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13]);
  expect(deltas(updates)).toEqual(sent.filter(isDeltaFrame));
  expect(updates.filter(Update.$is("Reset"))).toHaveLength(1);
  expect(updates.find(inState("Disconnected"))).toEqual(
    Update.Status({
      state: "Disconnected",
      error: "the connection closed: the network went away",
    }),
  );
});

test("on an epoch change, the link resets the host's history before the replay", async () => {
  const { updates, subscribes, reborn } = await scoped(
    Effect.gen(function* rebirth() {
      const [first, second] = [yield* Effect.promise(fakeHost), yield* Effect.promise(fakeHost)];
      const welcome = yield* turn.welcome(1);
      const restarted: Welcome = { ...welcome, epoch: `${welcome.epoch}0` };
      const feed = stored(turn.feed(1));
      yield* serve(first, { reply: { Welcome: welcome }, feed, results: [] });
      const again = yield* serve(second, {
        reply: { Welcome: restarted },
        feed,
        results: [],
      });
      const linked = yield* link(hosts([first, second]), ticket, client);
      yield* until(linked, inState("Live"));
      first.close("restarted");
      const seen = yield* until(linked, inState("Live"));
      return {
        updates: seen,
        subscribes: yield* Ref.get(again.subscribes),
        reborn: restarted,
      };
    }),
  );
  const reset = updates.findIndex(Update.$is("Reset"));
  expect(subscribes).toEqual([{ epoch: reborn.epoch, after_seq: 0 }]);
  expect(updates.slice(reset, reset + 2)).toEqual([
    Update.Reset({ epoch: reborn.epoch }),
    Update.Welcome({ welcome: reborn }),
  ]);
  expect(events(updates.slice(0, reset))).toEqual([]);
  expect(events(updates.slice(reset))).toEqual([1, 2, 3, 4, 5, 6, 7, 8]);
});

test("a reconnect drops the live connection and connects again at once, after the last applied seq", async () => {
  const { updates, subscribes, closed } = await scoped(
    Effect.gen(function* again() {
      const [first, second] = [yield* Effect.promise(fakeHost), yield* Effect.promise(fakeHost)];
      const replay = {
        reply: { Welcome: yield* turn.welcome(1) },
        feed: stored(turn.feed(1)),
        results: [],
      };
      yield* serve(first, replay);
      const next = yield* serve(second, replay);
      const linked = yield* link(hosts([first, second]), ticket, client);
      yield* until(linked, inState("Live"));
      yield* linked.reconnect;
      const seen = yield* until(linked, inState("Live"));
      const dropped = yield* Effect.promise(() => reach(first));
      return {
        updates: seen,
        subscribes: yield* Ref.get(next.subscribes),
        closed: yield* Effect.promise(() => dropped.closed()),
      };
    }),
  );
  expect(states(updates)).toEqual(["Disconnected", ...walking]);
  expect(updates.filter(Update.$is("Status")).map((update) => update.error)).toEqual([
    undefined,
    undefined,
    undefined,
    undefined,
    undefined,
  ]);
  expect(subscribes).toEqual([{ epoch: "1", after_seq: 8 }]);
  expect(events(updates)).toEqual([]);
  expect(closed).toBe("closed");
});

test("a refused hello drops the connection with the host's reason", async () => {
  const updates = await scoped(
    Effect.gen(function* refused() {
      const host = yield* Effect.promise(fakeHost);
      yield* serve(host, {
        reply: { Refused: { reason: "unknown token" } },
        feed: [],
        results: [],
      });
      const linked = yield* link(host.pipe, ticket, client);
      return yield* until(linked, inState("Disconnected"));
    }),
  );
  expect(updates.at(-1)).toEqual(
    Update.Status({
      state: "Disconnected",
      error: "the host refused us: unknown token",
    }),
  );
});
