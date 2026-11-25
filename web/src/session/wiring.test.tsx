import { Effect } from "effect";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "@effect/vitest";
import { fakeHost } from "#ui/test/fake-pipe.ts";
import { openRequests } from "@conductor/core/fold/offers";
import { scenario, serve } from "#ui/test/replay.ts";
import { Root } from "#ui/root.tsx";
import { Route } from "@conductor/core/route";
import { makeStore, sessionOf } from '#ui/store.ts';
import type { Store } from '#ui/store.ts';
import { page, when } from "#ui/test/page.ts";
import type { Feed } from "@conductor/core/schemas/index";

describe("wiring", () => {
  // Crates/link/src/ticket.rs's worked example, and the host it names.
  const ticket =
    "AFhmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmAQEBAQEBAQEBAQEBAQEBAWh0dHBzOi8vci5leGFtcGxlLw";
  const host = `58${"66".repeat(31)}`;

  const turn = scenario("a_claude_turn_streams_asks_and_finishes");
  const session = "00000000000000000000000001";

  // The turn's first connection: its frames for the events from `from` through `through`, with
  // Its frames that are not events when it starts from 0.
  const recordedSpan = (from: number, through: number): ReadonlyArray<Feed> =>
    turn.feed(0).filter((frame) => {
      if (!("Event" in frame)) {
        return from === 0;
      }
      return frame.Event.seq >= from && frame.Event.seq <= through;
    });

  const waiting = (store: Store): number => {
    const found = sessionOf(store.view(), host, session);
    return found === undefined ? -1 : openRequests(found).length;
  };

  it.effect(
    "the session route shows the store's session, and its approval follows the host's answer",
    () =>
      Effect.gen(function* () {
        expect.hasAssertions();
        const { before, after } = yield* Effect.scoped(
          Effect.gen(function* wired() {
            const fake = yield* Effect.tryPromise(fakeHost);
            const welcome = yield* turn.welcome(0);
            yield* serve(fake, {
              answers: [recordedSpan(9, 13)],
              feed: recordedSpan(0, 8),
              reply: { Welcome: welcome },
              results: [{ Session: "Accepted" }],
            });
            const store = yield* makeStore(page(`#join=${ticket}`, undefined, fake.pipe).boundary);
            yield* when(store, (shown) => waiting(shown) === 1);
            store.show(Route.Session({ host, session }));
            const shown = renderToStaticMarkup(<Root store={store} />);
            yield* store.send(host, {
              Session: { Resolve: { decision: "Approve", request: 1, session } },
            });
            yield* when(store, (next) => waiting(next) === 0);
            return {
              after: renderToStaticMarkup(<Root store={store} />),
              before: shown,
            };
          }),
        );
        expect(before).toContain('<div class="shell" data-top="pane">');
        expect(before).toContain('<p class="meta"><span>test-host</span>');
        expect(before).toContain('type="button">Approve</button>');
        expect(after).toContain("Approved by test-client");
      }),
  );
});
