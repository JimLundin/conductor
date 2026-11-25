import { Data, Result } from "effect";
import type { Ticket } from "#core/schemas/index";
import type { Unit } from "#core/data";
// A ticket's text form, as crates/link/src/ticket.rs writes it: base64url without padding of
// `version (1 byte) | host endpoint id (32) | token (16) | relay url (utf-8, rest)`. The
// Generated schemas carry only its JSON form, so this is the app's one reader of the text.

type TicketError = Data.TaggedEnum<{
  readonly Encoding: Unit;
  readonly Truncated: Unit;
  readonly Version: { readonly version: number };
  readonly RelayUrl: Unit;
}>;

const TicketError = Data.taggedEnum<TicketError>();

const version = 0;
const idLength = 32;
const tokenLength = 16;

const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
const bitsPerDigit = 6;
const bitsPerByte = 8;
const digitsPerGroup = 4;
const byteValues = 256;
const binaryRadix = 2;
const empty = 0;
const previous = 1;
const versionLength = 1;
const missing = -1;

// One group of up to four digits as its bytes, or undefined when a lone digit can't make a
// Byte or the bits after the last byte aren't zero, as the Rust decoder refuses.
const group = (digits: ReadonlyArray<number>): ReadonlyArray<number> | undefined => {
  const bits = digits.length * bitsPerDigit;
  const count = Math.floor(bits / bitsPerByte);
  const spare = bits - count * bitsPerByte;
  const value = digits.reduce((word, digit) => word * binaryRadix ** bitsPerDigit + digit, empty);
  if (count === empty || value % binaryRadix ** spare !== empty) {
    return undefined;
  }
  const word = Math.floor(value / binaryRadix ** spare);
  return Array.from(
    { length: count },
    (_unused, index) =>
      Math.floor(word / binaryRadix ** (bitsPerByte * (count - previous - index))) % byteValues,
  );
};

const unbase64url = (text: string): ReadonlyArray<number> | undefined => {
  const digits = text.split("").map((character) => alphabet.indexOf(character));
  if (digits.includes(missing)) {
    return undefined;
  }
  const groups = Array.from(
    { length: Math.ceil(digits.length / digitsPerGroup) },
    (_unused, index) =>
      group(digits.slice(index * digitsPerGroup, (index + previous) * digitsPerGroup)),
  );
  if (groups.includes(undefined)) {
    return undefined;
  }
  return groups.flatMap((bytes) => bytes ?? []);
};

const relayOf = (bytes: ReadonlyArray<number>): string | undefined => {
  const decoded = Result.try(() =>
    new TextDecoder("utf-8", { fatal: true }).decode(Uint8Array.from(bytes)),
  );
  const relay = Result.getOrUndefined(decoded);
  return relay !== undefined && URL.canParse(relay) ? relay : undefined;
};

const fields = (bytes: ReadonlyArray<number>): Result.Result<Ticket, TicketError> => {
  const relayAt = versionLength + idLength + tokenLength;
  if (bytes.length < relayAt) {
    return Result.fail(TicketError.Truncated());
  }
  const relay = relayOf(bytes.slice(relayAt));
  if (relay === undefined) {
    return Result.fail(TicketError.RelayUrl());
  }
  return Result.succeed({
    host: bytes.slice(versionLength, versionLength + idLength),
    relay,
    token: bytes.slice(versionLength + idLength, relayAt),
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
  return parseTicket(at === missing ? trimmed : trimmed.slice(at + joinMarker.length));
};

const describeTicketError = TicketError.$match({
  Encoding: () => "The ticket is not base64url.",
  RelayUrl: () => "The ticket's relay URL is invalid.",
  Truncated: () => "The ticket is truncated.",
  Version: ({ version: found }) => `Ticket version ${String(found)} is not supported.`,
});

export { describeTicketError, joinMarker, parseJoin, parseTicket, TicketError };
