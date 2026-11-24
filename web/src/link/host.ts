// What a link to one host keeps, and the updates it reports.
import { Data, Effect, Option, Queue, Ref, type Stream, type SubscriptionRef } from "effect";
import type { Connection, Pipe } from "../pipe.ts";
import type {
  ClientName,
  Command,
  CommandResult,
  Epoch,
  Feed,
  HostSeq,
  Ticket,
  Welcome,
} from "../schemas/index.ts";
import type { SendError } from "./command.ts";

type LinkState = "Disconnected" | "Connecting" | "Authenticating" | "CatchingUp" | "Live";

interface Status {
  readonly state: LinkState;
  // Why the last connection ended, until the next one is live.
  readonly error: string | undefined;
}

type Update = Data.TaggedEnum<{
  readonly Status: Status;
  // The events that follow belong to this epoch, starting after seq 0: anything folded from
  // an earlier epoch is stale.
  readonly Reset: { readonly epoch: Epoch };
  readonly Welcome: { readonly welcome: Welcome };
  readonly Feed: { readonly frame: Feed };
}>;

const Update = Data.taggedEnum<Update>();

interface HostLink {
  // Every status, reset, welcome and feed frame in order, for one reader.
  readonly updates: Stream.Stream<Update>;
  // Runs one command on its own stream. Waits while the link connects and fails at once while
  // it is down; the caller decides whether to retry.
  readonly send: (body: Command) => Effect.Effect<CommandResult, SendError>;
  // Drops the connection and connects again at once, as after a network change.
  readonly reconnect: Effect.Effect<void>;
}

interface Cursor {
  readonly epoch: Epoch;
  readonly seq: HostSeq;
}

// Where a command runs: on the connection once it is subscribed, nowhere while the link is
// down, and not yet while it connects.
type Access = Connection | "connecting" | "down";

interface Host {
  readonly pipe: Pipe;
  readonly ticket: Ticket;
  readonly client: ClientName;
  readonly updates: Queue.Queue<Update>;
  readonly status: Ref.Ref<Status>;
  readonly cursor: Ref.Ref<Option.Option<Cursor>>;
  readonly access: SubscriptionRef.SubscriptionRef<Access>;
}

type EventFrame = Extract<Feed, { readonly Event: unknown }>;

const emit = (host: Host, update: Update): Effect.Effect<void> =>
  Queue.offer(host.updates, update).pipe(Effect.asVoid);

const hasChanged = (before: Status, after: Status): boolean =>
  before.state !== after.state || before.error !== after.error;

// Shows `next` if it changed.
const publish = (host: Host, next: Status): Effect.Effect<void> =>
  Ref.getAndSet(host.status, next).pipe(
    Effect.flatMap((before) => {
      if (hasChanged(before, next)) {
        return emit(host, Update.Status(next));
      }
      return Effect.void;
    }),
  );

const show = (host: Host, state: LinkState): Effect.Effect<void> =>
  Ref.get(host.status).pipe(Effect.flatMap((status) => publish(host, { ...status, state })));

const appliedSeq = (host: Host): Effect.Effect<HostSeq> =>
  Ref.get(host.cursor).pipe(
    Effect.map((cursor) =>
      cursor.pipe(
        Option.map((at) => at.seq),
        Option.getOrElse(() => 0),
      ),
    ),
  );

export {
  type Access,
  appliedSeq,
  type Cursor,
  emit,
  type EventFrame,
  type Host,
  type HostLink,
  type LinkState,
  publish,
  show,
  type Status,
  Update,
};
