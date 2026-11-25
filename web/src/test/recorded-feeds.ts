/// <reference types="vite/client" />
import { CommandResult, Feed, HelloReply } from "@conductor/core/schemas/index";
import { Schema } from "effect";

// Recorded host feeds in web/fixtures/feeds/, for tests: each frame a host sent,
// On which of the test client's connections and streams.

const scenarioName = /^.*\/(?<name>[^/]+)\.json$/u;

const hello = Schema.Struct({
  connection: Schema.Number,
  frame: HelloReply,
  stream: Schema.Literal("hello"),
});

const command = Schema.Struct({
  connection: Schema.Number,
  frame: CommandResult,
  stream: Schema.Literal("command"),
});

const event = Schema.Struct({
  connection: Schema.Number,
  frame: Feed,
  stream: Schema.Literal("feed"),
});

const RecordedFeed = Schema.Array(Schema.Union([hello, command, event]));

type RecordedFeed = typeof RecordedFeed.Type;

// Each feed's JSON by its scenario's name.
const recordedFeeds = (): ReadonlyMap<string, unknown> =>
  new Map(
    Object.entries(
      import.meta.glob<unknown>("../../fixtures/feeds/*.json", {
        eager: true,
        import: "default",
      }),
    ).map(([path, feed]) => [path.replace(scenarioName, "$<name>"), feed]),
  );

const recordedFeed = (scenario: string): RecordedFeed =>
  Schema.decodeUnknownSync(RecordedFeed, { onExcessProperty: "error" })(
    recordedFeeds().get(scenario),
  );

export { RecordedFeed, recordedFeed, recordedFeeds };
