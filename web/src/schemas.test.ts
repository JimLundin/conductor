import { Schema } from "effect";
import { expect, test } from "vitest";
import { RecordedFeed, recordedFeeds } from "./recorded-feeds.ts";

test("every frame of the recorded host feeds decodes with the generated schemas", () => {
	const decode = Schema.decodeUnknownSync(RecordedFeed, {
		onExcessProperty: "error",
	});
	expect(recordedFeeds().size).toBeGreaterThan(0);
	for (const [scenario, feed] of recordedFeeds()) {
		expect(() => decode(feed), scenario).not.toThrow();
	}
});
