import { page, when } from "#ui/test/page.ts";
import { scenario, serve } from "#ui/test/replay.ts";
import { Effect } from "effect";
import { fakeHost } from "#ui/test/fake-pipe.ts";
import type { Feed } from "@conductor/core/schemas/index";
import { makeStore } from "#ui/store.ts";
import { openRequests } from "@conductor/core/fold/offers";
import { renderToStaticMarkup } from "react-dom/server";
import { Root } from "#ui/root.tsx";
import type { Scope } from "effect";
import { sessionOf } from "@conductor/core/store";
import type { Store } from "#ui/store.ts";

const ticket =
  "AFhmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmAQEBAQEBAQEBAQEBAQEBAWh0dHBzOi8vci5leGFtcGxlLw";
const host = `58${"66".repeat(31)}`;
const session = "00000000000000000000000001";

const recordedSpan = (from: number, through: number): ReadonlyArray<Feed> =>
  scenario("a_claude_turn_streams_asks_and_finishes")
    .feed(0)
    .filter((frame) => {
      if (!("Event" in frame)) {
        return from === 0;
      }
      return frame.Event.seq >= from && frame.Event.seq <= through;
    });

const waiting = (store: Store): number => {
  const found = sessionOf(store.view(), host, session);
  return found === undefined ? -1 : openRequests(found).length;
};

const wired = (): Effect.Effect<Store, never, Scope.Scope> =>
  Effect.gen(function* wiring() {
    const fake = yield* Effect.promise(fakeHost);
    const turn = scenario("a_claude_turn_streams_asks_and_finishes");
    const welcome = yield* turn.welcome(0);
    yield* serve(fake, {
      answers: [recordedSpan(9, 13)],
      feed: recordedSpan(0, 8),
      reply: { Welcome: welcome },
      results: [{ Session: "Accepted" }],
    });
    const browser = page(`#join=${ticket}`, undefined, fake.pipe);
    const store = yield* makeStore(browser.boundary);
    yield* when(store, (shown) => waiting(shown) === 1);
    browser.boundary.go(`#/host/${host}/session/${session}`);
    store.follow();
    return store;
  });

interface ApprovalMarkup {
  readonly after: string;
  readonly before: string;
}

const approvalThroughStore = (): Effect.Effect<ApprovalMarkup> =>
  Effect.scoped(
    Effect.gen(function* approve() {
      const store = yield* wired();
      const before = renderToStaticMarkup(<Root store={store} />);
      yield* store.send(host, {
        Session: { Resolve: { decision: "Approve", request: 1, session } },
      });
      yield* when(store, (next) => waiting(next) === 0);
      return { after: renderToStaticMarkup(<Root store={store} />), before };
    }),
  ).pipe(Effect.orDie);

export { approvalThroughStore };
