import { Result } from "effect";
import { expect, test } from "vitest";
import example from "../../crates/link/ticket.example.json" with {
	type: "json",
};
import { parseJoin, parseTicket, TicketError } from "./ticket.ts";

const { text, ticket } = example;

test("a ticket reads as the host wrote it", () => {
	expect(parseTicket(text)).toEqual(Result.succeed(ticket));
});

test("malformed tickets are refused", () => {
	expect(parseTicket("not base64!")).toEqual(
		Result.fail(TicketError.Encoding()),
	);
	expect(parseTicket(`${text}==`)).toEqual(Result.fail(TicketError.Encoding()));
	expect(parseTicket(text.slice(0, 20))).toEqual(
		Result.fail(TicketError.Truncated()),
	);
	expect(parseTicket(`B${text.slice(1)}`)).toEqual(
		Result.fail(TicketError.Version({ version: 4 })),
	);
	expect(parseTicket("")).toEqual(Result.fail(TicketError.Truncated()));
});

test("a join link or a bare ticket reads as the ticket, around any whitespace", () => {
	expect(
		parseJoin(`  https://jimlundin.github.io/conductor/#join=${text}\n`),
	).toEqual(Result.succeed(ticket));
	expect(parseJoin(text)).toEqual(Result.succeed(ticket));
	expect(
		Result.isFailure(parseJoin("https://jimlundin.github.io/conductor/")),
	).toBe(true);
});
