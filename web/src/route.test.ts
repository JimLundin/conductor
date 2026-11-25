import { Schema } from "effect";
import { describe, expect, it } from "@effect/vitest";
import { fragmentOf, isJoin, Route, route } from "@conductor/core/route";

describe("route", () => {
  it.prop(
    "with no host paired, every fragment shows pairing",
    { fragment: Schema.String },
    ({ fragment }) => route(`#${fragment}`, false)._tag === "Pairing",
  );

  it("each screen's fragment names that screen", () => {
    expect.hasAssertions();
    const screens = [
      Route.Pairing(),
      Route.Sessions(),
      Route.NewSession({ host: "ab12" }),
      Route.Session({ host: "ab12", session: "01JABCDEFGHJKMNPQRSTVWXYZ0" }),
    ];
    for (const screen of screens) {
      expect(route(fragmentOf(screen), true)).toStrictEqual(screen);
    }
    expect(fragmentOf(Route.Session({ host: "ab12", session: "s1" }))).toBe(
      "#/host/ab12/session/s1",
    );
  });

  it("with a host paired, an empty or unknown fragment shows the sessions", () => {
    expect.hasAssertions();
    expect(route("", true)._tag).toBe("Sessions");
    expect(route("#/host/ab12/elsewhere", true)._tag).toBe("Sessions");
    expect(route("#/host//new", true)._tag).toBe("Sessions");
  });

  it("a join link's fragment is told apart from a screen's", () => {
    expect.hasAssertions();
    expect(isJoin("#join=AAAA")).toBe(true);
    expect(isJoin("#/pair")).toBe(false);
  });
});
