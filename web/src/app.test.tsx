import { Effect } from "effect";
import { renderToStaticMarkup } from "react-dom/server";
import { expect, test } from "vitest";
import { dialing, scoped } from "./link/replay.ts";
import { memoryStorage } from "./memory-storage.ts";
import { Root } from "./root.tsx";
import { makeStore } from "./store.ts";

const rendered = (fragment: string): Promise<string> =>
	scoped(
		makeStore({
			pipe: dialing([]),
			client: "test-client",
			storage: memoryStorage(),
			fragment: (): string => fragment,
			go: (): void => undefined,
			replace: (): void => undefined,
		}).pipe(
			Effect.map((store) => renderToStaticMarkup(<Root store={store} />)),
		),
	);

test("the app opens on the empty pairing screen", async () => {
	expect(await rendered("")).toContain("<h1>Pair a host</h1>");
});
