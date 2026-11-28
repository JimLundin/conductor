import { Option, Schema } from "effect";
import { Ticket } from "#core/schemas/index";

type HostKey = string;

interface Saved {
  readonly ticket: Ticket;
  readonly name: string | undefined;
}

const SavedRecord = Schema.fromJsonString(
  Schema.Struct({ name: Schema.NullOr(Schema.String), ticket: Ticket }),
);

const prefix = "conductor.v2.host.";
const hexRadix = 16;
const hexDigits = 2;

const hostKey = (ticket: Ticket): HostKey =>
  ticket.host.map((byte) => byte.toString(hexRadix).padStart(hexDigits, "0")).join("");

const storageKey = (host: HostKey): string => `${prefix}${host}`;

const restored = (key: string, value: string | undefined): ReadonlyArray<Saved> => {
  if (!key.startsWith(prefix) || value === undefined) {
    return [];
  }
  return Schema.decodeUnknownOption(SavedRecord)(value).pipe(
    Option.filter((saved) => storageKey(hostKey(saved.ticket)) === key),
    Option.match({
      onNone: (): ReadonlyArray<Saved> => [],
      onSome: (saved): ReadonlyArray<Saved> => [
        { name: saved.name ?? undefined, ticket: saved.ticket },
      ],
    }),
  );
};

const savedText = (saved: Saved): string => {
  const name = Option.getOrNull(Option.fromUndefinedOr(saved.name));
  return Schema.encodeSync(SavedRecord)({ name, ticket: saved.ticket });
};

export { hostKey, restored, savedText, storageKey };
export type { HostKey, Saved };
