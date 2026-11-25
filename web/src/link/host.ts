import type { ClientName, Command, CommandResult, HostSeq, Ticket } from "@conductor/core/schemas/index";
import type { Connection, Pipe } from "#ui/pipe.ts";
import type { Cursor, LinkState, SendError, Status } from "@conductor/core/link";
import { Effect, Option, Queue, Ref } from "effect";
import type { Stream, SubscriptionRef } from "effect";
import { Update } from "@conductor/core/link";

interface HostLink {
  readonly reconnect: Effect.Effect<void>;
  readonly send: (body: Command) => Effect.Effect<CommandResult, SendError>;
  readonly updates: Stream.Stream<Update>;
}

type Send = HostLink["send"];
type Access = Connection | "connecting" | "down";

interface Host {
  readonly access: SubscriptionRef.SubscriptionRef<Access>;
  readonly client: ClientName;
  readonly cursor: Ref.Ref<Option.Option<Cursor>>;
  readonly pipe: Pipe;
  readonly status: Ref.Ref<Status>;
  readonly ticket: Ticket;
  readonly updates: Queue.Queue<Update>;
}

const firstSequence = 0;

const emit: (host: Host, update: Update) => Effect.Effect<void> = (host, update) =>
  Queue.offer(host.updates, update).pipe(Effect.asVoid);

const hasChanged: (before: Status, after: Status) => boolean = (before, after) =>
  before.state !== after.state || before.error !== after.error;

const publish: (host: Host, next: Status) => Effect.Effect<void> = (host, next) =>
  Ref.getAndSet(host.status, next).pipe(
    Effect.flatMap((before) => hasChanged(before, next) ? emit(host, Update.Status(next)) : Effect.void),
  );

const show: (host: Host, state: LinkState) => Effect.Effect<void> = (host, state) =>
  Ref.get(host.status).pipe(Effect.flatMap((status) => publish(host, { ...status, state })));

const appliedSeq: (host: Host) => Effect.Effect<HostSeq> = (host) =>
  Ref.get(host.cursor).pipe(
    Effect.map((cursor) => Option.getOrElse(Option.map(cursor, (value) => value.seq), () => firstSequence)),
  );

export { type Access, appliedSeq, emit, type Host, type HostLink, publish, type Send, show };
