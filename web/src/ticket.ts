// A ticket's text form, as crates/link/src/ticket.rs writes it: base64url without padding of
// `version (1 byte) | host endpoint id (32) | token (16) | relay url (utf-8, rest)`. The
// generated schemas carry only its JSON form, so this is the app's one reader of the text.
import { Data, Result } from "effect";
import type { Ticket } from "./schemas/index.ts";

type TicketError = Data.TaggedEnum<{
	readonly Encoding: Record<never, never>;
	readonly Truncated: Record<never, never>;
	readonly Version: { readonly version: number };
	readonly RelayUrl: Record<never, never>;
}>;

const TicketError = Data.taggedEnum<TicketError>();

const version = 0;
const idLength = 32;
const tokenLength = 16;

const alphabet =
	"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
const bitsPerDigit = 6;
const bitsPerByte = 8;
const digitsPerGroup = 4;
const byteMask = 0xff;

// One group of up to four digits as its bytes, or undefined when a lone digit can't make a
// byte or the bits after the last byte aren't zero, as the Rust decoder refuses.
const group = (digits: readonly number[]): readonly number[] | undefined => {
	const bits = digits.length * bitsPerDigit;
	const count = Math.floor(bits / bitsPerByte);
	const spare = bits - count * bitsPerByte;
	const value = digits.reduce(
		(word, digit) => word * 2 ** bitsPerDigit + digit,
		0,
	);
	if (count === 0 || value % 2 ** spare !== 0) {
		return undefined;
	}
	const word = Math.floor(value / 2 ** spare);
	return Array.from(
		{ length: count },
		(_, index) =>
			Math.floor(word / 2 ** (bitsPerByte * (count - 1 - index))) & byteMask,
	);
};

const unbase64url = (text: string): Uint8Array | undefined => {
	const digits = [...text].map((character) => alphabet.indexOf(character));
	if (digits.includes(-1)) {
		return undefined;
	}
	const groups = Array.from(
		{ length: Math.ceil(digits.length / digitsPerGroup) },
		(_, index) =>
			group(digits.slice(index * digitsPerGroup, (index + 1) * digitsPerGroup)),
	);
	if (groups.includes(undefined)) {
		return undefined;
	}
	return Uint8Array.from(groups.flatMap((bytes) => bytes ?? []));
};

const relayOf = (bytes: Uint8Array): string | undefined => {
	try {
		const relay = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
		return URL.canParse(relay) ? relay : undefined;
	} catch {
		return undefined;
	}
};

const fields = (bytes: Uint8Array): Result.Result<Ticket, TicketError> => {
	const relayAt = 1 + idLength + tokenLength;
	if (bytes.length < relayAt) {
		return Result.fail(TicketError.Truncated());
	}
	const relay = relayOf(bytes.subarray(relayAt));
	if (relay === undefined) {
		return Result.fail(TicketError.RelayUrl());
	}
	return Result.succeed({
		host: Array.from(bytes.subarray(1, 1 + idLength)),
		relay,
		token: Array.from(bytes.subarray(1 + idLength, relayAt)),
	});
};

const parseTicket = (text: string): Result.Result<Ticket, TicketError> => {
	const bytes = unbase64url(text.trim());
	if (bytes === undefined) {
		return Result.fail(TicketError.Encoding());
	}
	const [first] = bytes;
	if (first === undefined) {
		return Result.fail(TicketError.Truncated());
	}
	if (first !== version) {
		return Result.fail(TicketError.Version({ version: first }));
	}
	return fields(bytes);
};

// What a join link puts before its ticket; the daemon writes the same in crates/daemon/src/start.rs.
const joinMarker = "#join=";

// What a person pastes: a whole join link, or the bare ticket.
const parseJoin = (text: string): Result.Result<Ticket, TicketError> => {
	const trimmed = text.trim();
	const at = trimmed.lastIndexOf(joinMarker);
	return parseTicket(
		at === -1 ? trimmed : trimmed.slice(at + joinMarker.length),
	);
};

const describeTicketError = TicketError.$match({
	Encoding: () => "The ticket is not base64url.",
	Truncated: () => "The ticket is truncated.",
	Version: ({ version: found }) =>
		`Ticket version ${String(found)} is not supported.`,
	RelayUrl: () => "The ticket's relay URL is invalid.",
});

export { describeTicketError, joinMarker, parseJoin, parseTicket, TicketError };
