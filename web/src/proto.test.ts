import { Schema } from "effect";
import { expect, test } from "vitest";
import { CommandResult, Feed, HelloReply } from "./proto/index.ts";

const Recorded = Schema.Array(
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

const feeds = import.meta.glob<unknown>("../../fixtures/feeds/*.json", {
	eager: true,
	import: "default",
});

test("every frame of the recorded host feeds decodes with the generated schemas", () => {
	const decode = Schema.decodeUnknownSync(Recorded, {
		onExcessProperty: "error",
	});
	for (const [path, feed] of Object.entries(feeds)) {
		expect(() => decode(feed), path).not.toThrow();
	}
});
