import { Effect, Ref } from "effect";
import { client, dialing, inState, scenario, serve, ticket, until } from "#ui/test/replay.ts";
import { describe, expect, it } from "@effect/vitest";
import type { Command } from "@conductor/core/schemas/index";
import type { Connection } from "#ui/pipe.ts";
import { SendError } from "@conductor/core/link";
import { fakeHost } from "#ui/test/fake-pipe.ts";
import { link } from "#ui/link.ts";

const turn = (): ReturnType<typeof scenario> => scenario("a_claude_turn_streams_asks_and_finishes");

const interrupt: Command = {
  Session: { Interrupt: { session: "00000000000000000000000001" } },
} as const;

const unreachable = (): Promise<Connection> => Promise.reject(new Error("no route"));

const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u;

describe("command correlation", () => {
  it.effect("each command gets its own stream and returns its own result to its caller", () =>
    Effect.gen(function* verify1() {
      const results = turn().results(0);
      const { answers, commands } = yield* Effect.scoped(
        Effect.gen(function* commanding() {
          const host = yield* Effect.tryPromise(fakeHost);
          const reply = { Welcome: yield* turn().welcome(0) };
          const served = yield* serve(host, { feed: turn().feed(0), reply, results });
          const linked = yield* link(host.pipe, ticket, client);
          yield* until(linked, inState("Live"));
          const answered = yield* Effect.forEach(results, () => linked.send(interrupt));
          return { answers: answered, commands: yield* Ref.get(served.commands) };
        }),
      );
      expect(answers).toStrictEqual(results);
      expect(commands.map((command) => command.body)).toStrictEqual(results.map(() => interrupt));
      expect(new Set(commands.map((command) => command.id)).size).toBe(results.length);
      expect(commands.filter((command) => uuid.test(command.id))).toStrictEqual(commands);
    }),
  );
});

describe("disconnected commands", () => {
  it.effect("a command while the link is down fails as not connected", () =>
    Effect.gen(function* verify2() {
      const failed = yield* Effect.scoped(
        Effect.gen(function* down() {
          const linked = yield* link(dialing([unreachable]), ticket, client);
          yield* until(linked, inState("Disconnected"));
          return yield* Effect.flip(linked.send(interrupt));
        }),
      );
      expect(failed).toStrictEqual(SendError.NotConnected());
    }),
  );
});

describe("missing command results", () => {
  it.effect("a command whose stream ends without a result fails as such", () =>
    Effect.gen(function* verify3() {
      const failed = yield* Effect.scoped(
        Effect.gen(function* unanswered() {
          const host = yield* Effect.tryPromise(fakeHost);
          yield* serve(host, {
            feed: turn().feed(0),
            reply: { Welcome: yield* turn().welcome(0) },
            results: [],
          });
          const linked = yield* link(host.pipe, ticket, client);
          yield* until(linked, inState("Live"));
          return yield* Effect.flip(linked.send(interrupt));
        }),
      );
      expect(failed).toStrictEqual(SendError.NoResult());
    }),
  );
});
