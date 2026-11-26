import { Route, fragmentOf, isJoin, route } from "@conductor/core/route";
import { describe, expect, it } from "@effect/vitest";
import { Schema } from "effect";

describe(route, () => {
  it.prop("shows pairing without a paired host", { fragment: Schema.String }, ({ fragment }) =>
    Route.$is("Pairing")(route(`#${fragment}`, false)),
  );

  it.each(["", "#/host/ab12/elsewhere", "#/host//new"])(
    "shows the sessions for an unknown fragment %s",
    (fragment) => {
      expect.hasAssertions();
      expect(route(fragment, true)).toStrictEqual(Route.Sessions());
    },
  );
});

describe(fragmentOf, () => {
  it.each([
    [Route.Pairing(), "#/pair"],
    [Route.Sessions(), "#/"],
    [Route.Work({ issue: "", project: "" }), "#/new"],
    [Route.Work({ issue: "", project: "hyn162" }), "#/project/hyn162"],
    [
      Route.Work({ issue: "github%2Forg~42", project: "hyn162" }),
      "#/project/hyn162/issue/github%2Forg~42",
    ],
    [Route.NewSession({ host: "ab12" }), "#/host/ab12/new"],
    [
      Route.Session({ host: "ab12", session: "01JABCDEFGHJKMNPQRSTVWXYZ0" }),
      "#/host/ab12/session/01JABCDEFGHJKMNPQRSTVWXYZ0",
    ],
    [Route.Session({ host: "ab12", session: "s1" }), "#/host/ab12/session/s1"],
  ] as const)("round-trips a screen as %s", (screen, fragment) => {
    expect.hasAssertions();
    expect(fragmentOf(screen)).toBe(fragment);
    expect(route(fragment, true)).toStrictEqual(screen);
  });
});

describe(isJoin, () => {
  it.each([
    ["#join=AAAA", true],
    ["#/pair", false],
  ] as const)("recognizes a join fragment %s as %s", (fragment, expected) => {
    expect.hasAssertions();
    expect(isJoin(fragment)).toBe(expected);
  });
});
