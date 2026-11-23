import { renderToStaticMarkup } from "react-dom/server";
import { expect, test } from "vitest";
import { App } from "./app.tsx";
import { route } from "./route.ts";

test("the app opens on the empty pairing screen", () => {
	const html = renderToStaticMarkup(<App route={route("")} />);
	expect(html).toContain("<h1>Pair a host</h1>");
});
