import { Result } from "effect";
import { expect, test } from "vitest";
import { parseJoin, parseTicket, TicketError } from "./ticket.ts";

// crates/host/src/fleet/ticket.rs's worked example.
const text =
	"AFhmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmAQEBAQEBAQEBAQEBAQEBAWh0dHBzOi8vci5leGFtcGxlLw";
const ticket = {
	host: [0x58, ...Array.from({ length: 31 }, () => 0x66)],
	relay: "https://r.example/",
	token: Array.from({ length: 16 }, () => 1),
} as const;

test("a ticket's text reads as host's worked example", () => {
	expect(parseTicket(text)).toEqual(Result.succeed(ticket));
});

test("malformed tickets are refused as host refuses them", () => {
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
