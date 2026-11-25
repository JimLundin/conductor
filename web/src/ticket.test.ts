import { TicketError, parseJoin, parseTicket } from "@conductor/core/ticket";
import { describe, expect, it } from "@effect/vitest";
import { Result } from "effect";
import example from "#ticket-example" with { type: "json" };

const { text, ticket } = example;

describe(parseTicket, () => {
  it("reads a ticket as the host wrote it", () => {
    expect.hasAssertions();
    expect(parseTicket(text)).toStrictEqual(Result.succeed(ticket));
  });

  it.each([
    ["not base64!", TicketError.Encoding()],
    [`${text}==`, TicketError.Encoding()],
    [text.slice(0, 20), TicketError.Truncated()],
    [`B${text.slice(1)}`, TicketError.Version({ version: 4 })],
    ["", TicketError.Truncated()],
  ] as const)("refuses a malformed ticket %s", (input, error) => {
    expect.hasAssertions();
    expect(parseTicket(input)).toStrictEqual(Result.fail(error));
  });
});

describe(parseJoin, () => {
  it.each([`  https://jimlundin.github.io/conductor/#join=${text}\n`, text])(
    "reads a join link or bare ticket %s",
    (input) => {
      expect.hasAssertions();
      expect(parseJoin(input)).toStrictEqual(Result.succeed(ticket));
    },
  );

  it("refuses a URL with no join fragment", () => {
    expect.hasAssertions();
    const parsed = parseJoin("https://jimlundin.github.io/conductor/");
    expect(Result.isFailure(parsed)).toBe(true);
  });
});
