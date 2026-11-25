import type { Connection, Stream } from "#ui/pipe.ts";
import { Effect, Option, Ref, SubscriptionRef } from "effect";
import type { Epoch, HostSeq, Welcome } from "@conductor/core/schemas/index";
import { Feed, Open } from "@conductor/core/schemas/index";
import { appliedSeq, emit, publish } from "#ui/link/host.ts";
import { caughtUp, eventActions, sameEpoch } from "@conductor/core/runtime/feed";
import { open, read, write } from "#ui/link/frames.ts";
import type { EventFrame } from "@conductor/core/link";
import type { FeedAction } from "@conductor/core/runtime/feed";
import type { Host } from "#ui/link/host.ts";
import { Update } from "@conductor/core/link";

const adopt: (host: Host, epoch: Epoch) => Effect.Effect<void> = (host, epoch) =>
  Ref.get(host.cursor).pipe(
    Effect.flatMap((cursor) => {
      if (sameEpoch(Option.getOrUndefined(cursor), epoch)) {
        return Effect.void;
      }
      const reset = Option.some({ epoch, seq: 0 });
      return Ref.set(host.cursor, reset).pipe(
        Effect.andThen(emit(host, Update.Reset({ epoch }))),
      );
    }),
  );

const catchUp: (host: Host, applied: HostSeq, liveAt: HostSeq) => Effect.Effect<void> = (host, applied, liveAt) =>
  Ref.get(host.status).pipe(
    Effect.flatMap((status) =>
      caughtUp(status, applied, liveAt)
        ? publish(host, { error: undefined, state: "Live" })
        : Effect.void,
    ),
  );

const execute: (host: Host, action: FeedAction) => Effect.Effect<void> = (host, action) => {
  if (action.kind === "Cursor") {
    return Ref.set(host.cursor, Option.some(action.cursor));
  }
  return emit(host, Update.Feed({ frame: action.frame }));
};

const applyEvent: (host: Host, welcome: Welcome, frame: EventFrame) => Effect.Effect<void, string> = (host, welcome, frame) =>
  Effect.gen(function* applying() {
    const after = yield* appliedSeq(host);
    const actions = yield* Effect.fromResult(eventActions(welcome, frame, after));
    yield* Effect.forEach(actions, (action) => execute(host, action), { discard: true });
    const applied = yield* appliedSeq(host);
    yield* catchUp(host, applied, welcome.head_seq);
  });

const apply: (host: Host, welcome: Welcome, frame: Feed) => Effect.Effect<void, string> = (host, welcome, frame) => {
  if (!("Event" in frame)) {
    return emit(host, Update.Feed({ frame }));
  }
  return applyEvent(host, welcome, frame);
};

const nextFrame: (stream: Stream) => Effect.Effect<Feed, string> = (stream) =>
  read(Feed, stream).pipe(
    Effect.flatMap((frame) =>
      frame === undefined
        ? Effect.fail("the host closed the subscription stream")
        : Effect.succeed(frame),
    ),
  );

const subscribe: (host: Host, connection: Connection, welcome: Welcome) => Effect.Effect<never, string> = (
  host,
  connection,
  welcome,
) =>
  Effect.gen(function* subscription() {
    const stream = yield* open(connection);
    const after = yield* appliedSeq(host);
    yield* write(Open, stream, { Subscribe: { after_seq: after, epoch: welcome.epoch } });
    const first = yield* nextFrame(stream);
    yield* SubscriptionRef.set(host.access, connection);
    yield* catchUp(host, after, welcome.head_seq);
    yield* apply(host, welcome, first);
    return yield* Effect.forever(
      nextFrame(stream).pipe(Effect.flatMap((frame) => apply(host, welcome, frame))),
    );
  });

export { adopt, subscribe };
