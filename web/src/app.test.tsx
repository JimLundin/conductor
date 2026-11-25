import { describe, expect, it } from "@effect/vitest";
import { page, when } from "#ui/test/page.ts";
import { scenario, serve } from "#ui/test/replay.ts";
import { Effect } from "effect";
import type { Pipe } from "#ui/pipe.ts";
import { Root } from "#ui/root.tsx";
import type { Scope } from "effect";
import { fakeHost } from "#ui/test/fake-pipe.ts";
import { makeStore } from "#ui/store.ts";
import { renderToStaticMarkup } from "react-dom/server";
import { sessionsOf } from "@conductor/core/store";

const ticket =
  "AFhmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmAQEBAQEBAQEBAQEBAQEBAWh0dHBzOi8vci5leGFtcGxlLw";
const host = `58${"66".repeat(31)}`;

const rendered = (fragment: string, pipe?: Pipe): Effect.Effect<string, never, Scope.Scope> =>
  Effect.gen(function* rendering() {
    const store = yield* makeStore(page(fragment, undefined, pipe).boundary);
    return renderToStaticMarkup(<Root store={store} />);
  });

const turn = (): ReturnType<typeof scenario> => scenario("a_claude_turn_streams_asks_and_finishes");

describe("empty app", () => {
  it.effect("the app opens on the empty pairing screen", () =>
    Effect.gen(function* verify1() {
      expect(yield* Effect.scoped(rendered(""))).toContain("<h1>Pair a host</h1>");
    }),
  );
});

describe("host sessions", () => {
  it.effect(
    "the session list shows each host's name and link state, and its sessions with their status",
    () =>
      Effect.gen(function* verify2() {
        const { html, name } = yield* Effect.scoped(
          Effect.gen(function* listing() {
            const fake = yield* Effect.tryPromise(fakeHost);
            const welcome = yield* turn().welcome(1);
            const feed = turn().feed(1);
            yield* serve(fake, { feed, reply: { Welcome: welcome }, results: [] });
            const store = yield* makeStore(page(`#join=${ticket}`, undefined, fake.pipe).boundary);
            yield* when(
              store,
              (shown) =>
                shown.view().hosts.get(host)?.status.state === "Live" &&
                sessionsOf(shown.view(), host).length > 0,
            );
            return {
              html: renderToStaticMarkup(<Root store={store} />),
              name: welcome.host,
            };
          }),
        );
        expect(html).toContain(`<h2>${name}</h2>`);
        expect(html).toContain('data-state="Live"');
        expect(html).toContain(`href="#/host/${host}/session/00000000000000000000000001"`);
        expect(html).toMatch(
          /class="session-status" data-status="(?<status>Idle|Running|Closed|Requests)"/v,
        );
      }),
  );
});
