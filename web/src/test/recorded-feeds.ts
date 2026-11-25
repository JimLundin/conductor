import { CommandResult, Feed, HelloReply } from "@conductor/core/schemas/index";
import { Schema } from "effect";

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

const recordedFeeds = (): ReadonlyMap<string, unknown> =>
  new Map(
    Object.entries(
      import.meta.glob<unknown>("../../fixtures/feeds/*.json", {
        eager: true,
        import: "default",
      }),
    ).map(([path, feed]: readonly [string, unknown]) => [
      path.replace(scenarioName, "$<name>"),
      feed,
    ]),
  );

const recordedFeed = (scenario: string): RecordedFeed =>
  Schema.decodeUnknownSync(RecordedFeed, { onExcessProperty: "error" })(
    recordedFeeds().get(scenario),
  );

export { RecordedFeed, recordedFeed, recordedFeeds };
