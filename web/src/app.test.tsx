import { Effect, type Scope } from "effect";
import { renderToStaticMarkup } from "react-dom/server";
import { expect, test } from "vitest";
import { fakeHost } from "./fake-pipe.ts";
import { scenario, scoped, serve } from "./link/replay.ts";
import type { Pipe } from "./pipe.ts";
import { Root } from "./root.tsx";
import { makeStore, sessionsOf } from "./store.ts";
import { page, when } from "./test-page.ts";

const ticket =
  "AFhmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmAQEBAQEBAQEBAQEBAQEBAWh0dHBzOi8vci5leGFtcGxlLw";
const host = `58${"66".repeat(31)}`;

const rendered = (fragment: string, pipe?: Pipe): Effect.Effect<string, never, Scope.Scope> =>
  makeStore(page(fragment, undefined, pipe).boundary).pipe(
    Effect.map((store) => renderToStaticMarkup(<Root store={store} />)),
  );

test("the app opens on the empty pairing screen", async () => {
  expect(await scoped(rendered(""))).toContain("<h1>Pair a host</h1>");
});

const turn = scenario("a_claude_turn_streams_asks_and_finishes");

test("the session list shows each host's name and link state, and its sessions with their status", async () => {
  const { html, name } = await scoped(
    Effect.gen(function* listing() {
      const fake = yield* Effect.promise(fakeHost);
      const welcome = yield* turn.welcome(1);
      const feed = turn.feed(1);
      yield* serve(fake, { reply: { Welcome: welcome }, feed, results: [] });
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
  expect(html).toMatch(/class="session-status" data-status="(Idle|Running|Closed|Requests)"/v);
});
