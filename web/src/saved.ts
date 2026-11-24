// The paired hosts as local storage keeps them: one record per host, under a key holding the
// format's version and the host's endpoint id in hex, so tabs pair and forget hosts each on
// their own. Records in any other format, the Dioxus app's included, are set aside unread.
import { Option, Schema } from "effect";
import { Ticket } from "./schemas/index.ts";

// A host's endpoint id in lower-case hex, which keys it in storage and in the page.
type HostKey = string;

interface Saved {
  readonly ticket: Ticket;
  // The name from its last welcome, kept across launches.
  readonly name: string | null;
}

const SavedRecord = Schema.fromJsonString(
  Schema.Struct({ ticket: Ticket, name: Schema.NullOr(Schema.String) }),
);

const prefix = "conductor.v2.host.";
const hexRadix = 16;
const hexDigits = 2;

const hostKey = (ticket: Ticket): HostKey =>
  ticket.host.map((byte) => byte.toString(hexRadix).padStart(hexDigits, "0")).join("");

const storageKey = (host: HostKey): string => `${prefix}${host}`;

// A record that doesn't decode, or whose ticket names another host than its key, is skipped
// without discarding its neighbours.
const restored = (key: string, value: string | null): readonly Saved[] => {
  if (!key.startsWith(prefix) || value === null) {
    return [];
  }
  return Schema.decodeUnknownOption(SavedRecord)(value).pipe(
    Option.filter((saved) => storageKey(hostKey(saved.ticket)) === key),
    Option.match({
      onNone: (): readonly Saved[] => [],
      onSome: (saved): readonly Saved[] => [saved],
    }),
  );
};

const keys = (storage: Storage): readonly string[] =>
  Array.from({ length: storage.length }, (_, index) => storage.key(index) ?? "");

// The hosts paired in `storage`.
const restore = (storage: Storage): readonly Saved[] =>
  keys(storage).flatMap((key) => restored(key, storage.getItem(key)));

const save = (storage: Storage, saved: Saved): void => {
  storage.setItem(storageKey(hostKey(saved.ticket)), Schema.encodeSync(SavedRecord)(saved));
};

const unsave = (storage: Storage, host: HostKey): void => {
  storage.removeItem(storageKey(host));
};

export { type HostKey, hostKey, restore, save, unsave };
