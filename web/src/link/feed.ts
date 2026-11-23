// A subscription's frames, applied once each and in order after the link's cursor.
import { Effect, Option, Ref, SubscriptionRef } from "effect";
import { isDelta } from "../fold/host.ts";
import type { Connection } from "../pipe.ts";
import {
	type Epoch,
	Feed,
	type HostSeq,
	type Welcome,
} from "../proto/index.ts";
import { open, read, write } from "./frames.ts";
import {
	appliedSeq,
	emit,
	type EventFrame,
	type Host,
	publish,
	Update,
} from "./host.ts";

// Keeps the cursor if the host's log is the one it points into; otherwise starts over from the
// beginning of the new epoch.
const adopt = (host: Host, epoch: Epoch): Effect.Effect<void> =>
	Ref.get(host.cursor).pipe(
		Effect.flatMap((cursor) => {
			if (Option.isSome(cursor) && cursor.value.epoch === epoch) {
				return Effect.void;
			}
			return Ref.set(host.cursor, Option.some({ epoch, seq: 0 })).pipe(
				Effect.andThen(emit(host, Update.Reset({ epoch }))),
			);
		}),
	);

// Goes live once the link has applied the host's head as of the welcome.
const catchUp = (
	host: Host,
	applied: HostSeq,
	liveAt: HostSeq,
): Effect.Effect<void> =>
	Ref.get(host.status).pipe(
		Effect.flatMap((status) => {
			if (applied < liveAt || status.state !== "CatchingUp") {
				return Effect.void;
			}
			return publish(host, { state: "Live", error: undefined });
		}),
	);

// Forwards each stored event once and in order, and each delta of the event applied last. A
// stored event already applied is a replay and dropped; one past the next is a gap.
const applyEvent = (
	host: Host,
	welcome: Welcome,
	frame: EventFrame,
	after: HostSeq,
): Effect.Effect<void, string> => {
	const { seq } = frame.Event;
	const delta = isDelta(frame.Event);
	const forward = emit(host, Update.Feed({ frame }));
	if (seq <= after && delta) {
		return forward;
	}
	if (seq <= after) {
		return Effect.void;
	}
	if (seq > after + 1 || delta) {
		return Effect.fail(
			`event ${seq} arrived after ${after}: the feed has a gap`,
		);
	}
	return Ref.set(host.cursor, Option.some({ epoch: welcome.epoch, seq })).pipe(
		Effect.andThen(forward),
		Effect.andThen(catchUp(host, seq, welcome.head_seq)),
	);
};

const apply = (
	host: Host,
	welcome: Welcome,
	frame: Feed,
): Effect.Effect<void, string> => {
	if (!("Event" in frame)) {
		return emit(host, Update.Feed({ frame }));
	}
	return appliedSeq(host).pipe(
		Effect.flatMap((after) => applyEvent(host, welcome, frame, after)),
	);
};

const nextFrame = (
	stream: Parameters<typeof read>[1],
): Effect.Effect<Feed, string> =>
	read(Feed, stream).pipe(
		Effect.flatMap((frame) => {
			if (frame === undefined) {
				return Effect.fail("the host closed the subscription stream");
			}
			return Effect.succeed(frame);
		}),
	);

const subscribe = (
	host: Host,
	connection: Connection,
	welcome: Welcome,
): Effect.Effect<never, string> =>
	Effect.gen(function* subscription() {
		const stream = yield* open(connection);
		const after = yield* appliedSeq(host);
		yield* write(stream, {
			Subscribe: { epoch: welcome.epoch, after_seq: after },
		});
		// The host always sends a first frame once it accepts the subscription.
		const first = yield* nextFrame(stream);
		yield* SubscriptionRef.set(host.access, connection);
		yield* catchUp(host, after, welcome.head_seq);
		yield* apply(host, welcome, first);
		return yield* Effect.forever(
			nextFrame(stream).pipe(
				Effect.flatMap((frame) => apply(host, welcome, frame)),
			),
		);
	});

export { adopt, subscribe };
