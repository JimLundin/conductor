import fc from "fast-check";
import { expect, test } from "vitest";
import { route } from "./route.ts";

test("with no host paired, every fragment shows pairing", () => {
	fc.assert(
		fc.property(fc.string(), (fragment) => {
			expect(route(`#${fragment}`)._tag).toBe("Pairing");
		}),
	);
	expect(route("")._tag).toBe("Pairing");
});
