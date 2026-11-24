import { Effect } from "effect";
import { renderToStaticMarkup } from "react-dom/server";
import { expect, test } from "vitest";
import { fakeHost } from "../fake-pipe.ts";
import { openRequests } from "../fold/offers.ts";
import { scenario, scoped, serve } from "../link/replay.ts";
import { Root } from "../root.tsx";
import { Route } from "../route.ts";
import { makeStore, type Store, sessionOf } from "../store.ts";
import { page, when } from "../test-page.ts";
import type { Feed } from "../schemas/index.ts";

// crates/link/src/ticket.rs's worked example, and the host it names.
const ticket =
	"AFhmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmAQEBAQEBAQEBAQEBAQEBAWh0dHBzOi8vci5leGFtcGxlLw";
const host = `58${"66".repeat(31)}`;

const turn = scenario("a_claude_turn_streams_asks_and_finishes");
const session = "00000000000000000000000001";

// The turn's first connection: its frames for the events from `from` through `through`, with
// its frames that are not events when it starts from 0.
const recordedSpan = (from: number, through: number): readonly Feed[] =>
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

test("the session route shows the store's session, and its approval follows the host's answer", async () => {
	const { before, after } = await scoped(
		Effect.gen(function* wired() {
			const fake = yield* Effect.promise(fakeHost);
			const welcome = yield* turn.welcome(0);
			yield* serve(fake, {
				reply: { Welcome: welcome },
				feed: recordedSpan(0, 8),
				results: [{ Session: "Accepted" }],
				answers: [recordedSpan(9, 13)],
			});
			const store = yield* makeStore(
				page(`#join=${ticket}`, undefined, fake.pipe).boundary,
			);
			yield* when(store, (shown) => waiting(shown) === 1);
			store.show(Route.Session({ host, session }));
			const shown = renderToStaticMarkup(<Root store={store} />);
			yield* store.send(host, {
				Session: { Resolve: { session, request: 1, decision: "Approve" } },
			});
			yield* when(store, (next) => waiting(next) === 0);
			return {
				before: shown,
				after: renderToStaticMarkup(<Root store={store} />),
			};
		}),
	);
	expect(before).toContain('<div class="shell" data-top="pane">');
	expect(before).toContain('<p class="meta"><span>test-host</span>');
	expect(before).toContain('type="button">Approve</button>');
	expect(after).toContain("Approved by test-client");
});
