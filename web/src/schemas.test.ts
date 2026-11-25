import { Schema } from "effect";
import { describe, expect, it } from "@effect/vitest";
import { RecordedFeed, recordedFeeds } from "#ui/test/recorded-feeds.ts";

describe("schemas", () => {
  it("every frame of the recorded host feeds decodes with the generated schemas", () => {
    expect.hasAssertions();
    const decode = Schema.decodeUnknownSync(RecordedFeed, {
      onExcessProperty: "error",
    });
    expect(recordedFeeds().size).toBeGreaterThan(0);
    for (const [scenario, feed] of recordedFeeds()) {
      expect(() => decode(feed), scenario).not.toThrow();
    }
  });
});
