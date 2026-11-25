import { RecordedFeed, recordedFeeds } from "#ui/test/recorded-feeds.ts";
import { Result, Schema } from "effect";
import { describe, expect, it } from "@effect/vitest";

describe("the recorded host feeds", () => {
  it("contains recorded scenarios", () => {
    expect.hasAssertions();
    expect(recordedFeeds().size).toBeGreaterThan(0);
  });

  it.each([...recordedFeeds()])("decodes every frame of %s", (_scenario, feed) => {
    expect.hasAssertions();
    const decode = Schema.decodeUnknownResult(RecordedFeed, { onExcessProperty: "error" });
    expect(Result.isSuccess(decode(feed))).toBe(true);
  });
});
