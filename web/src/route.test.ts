import fc from "fast-check";
import { expect, test } from "vitest";
import { fragmentOf, isJoin, Route, route } from "./route.ts";

test("with no host paired, every fragment shows pairing", () => {
	fc.assert(
		fc.property(fc.string(), (fragment) => {
			expect(route(`#${fragment}`, false)._tag).toBe("Pairing");
		}),
	);
	expect(route("", false)._tag).toBe("Pairing");
});

test("each screen's fragment names that screen", () => {
	const screens = [
		Route.Pairing(),
		Route.Sessions(),
		Route.NewSession({ host: "ab12" }),
		Route.Session({ host: "ab12", session: "01jabcdefghjkmnpqrstvwxyz0" }),
	];
	for (const screen of screens) {
		expect(route(fragmentOf(screen), true)).toEqual(screen);
	}
	expect(fragmentOf(Route.Session({ host: "ab12", session: "s1" }))).toBe(
		"#/host/ab12/session/s1",
	);
});

test("with a host paired, an empty or unknown fragment shows the sessions", () => {
	expect(route("", true)._tag).toBe("Sessions");
	expect(route("#/host/ab12/elsewhere", true)._tag).toBe("Sessions");
	expect(route("#/host//new", true)._tag).toBe("Sessions");
});

test("a join link's fragment is told apart from a screen's", () => {
	expect(isJoin("#join=AAAA")).toBe(true);
	expect(isJoin("#/pair")).toBe(false);
});
