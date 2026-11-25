import { Result } from "effect";
import { describe, expect, it } from "@effect/vitest";
import example from "#ticket-example" with { type: "json" };
import { parseJoin, parseTicket, TicketError } from "@conductor/core/ticket";

describe("ticket", () => {
  const { text, ticket } = example;

  it("a ticket reads as the host wrote it", () => {
    expect.hasAssertions();
    expect(parseTicket(text)).toStrictEqual(Result.succeed(ticket));
  });

  it("malformed tickets are refused", () => {
    expect.hasAssertions();
    expect(parseTicket("not base64!")).toStrictEqual(Result.fail(TicketError.Encoding()));
    expect(parseTicket(`${text}==`)).toStrictEqual(Result.fail(TicketError.Encoding()));
    expect(parseTicket(text.slice(0, 20))).toStrictEqual(Result.fail(TicketError.Truncated()));
    expect(parseTicket(`B${text.slice(1)}`)).toStrictEqual(
      Result.fail(TicketError.Version({ version: 4 })),
    );
    expect(parseTicket("")).toStrictEqual(Result.fail(TicketError.Truncated()));
  });

  it("a join link or a bare ticket reads as the ticket, around any whitespace", () => {
    expect.hasAssertions();
    expect(parseJoin(`  https://jimlundin.github.io/conductor/#join=${text}\n`)).toStrictEqual(
      Result.succeed(ticket),
    );
    expect(parseJoin(text)).toStrictEqual(Result.succeed(ticket));
    expect(Result.isFailure(parseJoin("https://jimlundin.github.io/conductor/"))).toBe(true);
  });
});
