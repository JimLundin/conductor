import { Effect } from "effect";
import { expect, test } from "vitest";
import { canonical, mint } from "./ulid.ts";

const written = /^[0-7][0-9A-HJKMNP-TV-Z]{25}$/u;

test("an id is read in either case and kept in the upper case the host writes", () => {
	expect(canonical("01jq8zk3vyn7m2wxr4tbhc5de1")).toBe(
		"01JQ8ZK3VYN7M2WXR4TBHC5DE1",
	);
	expect(canonical("01JQ8zk3VYN7M2WXR4TBHC5DE1")).toBe(
		"01JQ8ZK3VYN7M2WXR4TBHC5DE1",
	);
	expect(canonical("7ZZZZZZZZZZZZZZZZZZZZZZZZZ")).toBe(
		"7ZZZZZZZZZZZZZZZZZZZZZZZZZ",
	);
});

test("text that is no id is refused", () => {
	expect(canonical("")).toBeUndefined();
	expect(canonical("01JQ8ZK3VYN7M2WXR4TBHC5DE")).toBeUndefined();
	expect(canonical("01JQ8ZK3VYN7M2WXR4TBHC5DE12")).toBeUndefined();
	expect(canonical("01JQ8ZK3VYN7M2WXR4TBHC5DEU")).toBeUndefined();
	expect(canonical("01JQ8ZK3VYN7M2WXR4TBHC5DEI")).toBeUndefined();
	expect(canonical("80000000000000000000000000")).toBeUndefined();
});

test("a minted id is fresh and written as the host writes ids", async () => {
	const minted = await Effect.runPromise(
		Effect.all(Array.from({ length: 64 }, () => mint)),
	);
	expect(minted.filter((id) => written.test(id))).toEqual(minted);
	expect(new Set(minted).size).toBe(minted.length);
});
