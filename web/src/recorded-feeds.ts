/// <reference types="vite/client" />
// The daemon e2e's recorded host feeds in fixtures/feeds/, for tests: each frame a host sent,
// on which of the test client's connections and streams.
import { Schema } from "effect";
import { CommandResult, Feed, HelloReply } from "./proto/index.ts";

const scenarioName = /^.*\/(?<name>[^/]+)\.json$/u;

const RecordedFeed = Schema.Array(
	Schema.Union([
		Schema.Struct({
			connection: Schema.Number,
			stream: Schema.Literal("hello"),
			frame: HelloReply,
		}),
		Schema.Struct({
			connection: Schema.Number,
			stream: Schema.Literal("command"),
			frame: CommandResult,
		}),
		Schema.Struct({
			connection: Schema.Number,
			stream: Schema.Literal("feed"),
			frame: Feed,
		}),
	]),
);

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
