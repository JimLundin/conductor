import type { Cursor, EventFrame, Status } from "@conductor/core/link";
import type { Epoch, Feed, HostSeq, Welcome } from "@conductor/core/schemas/index";
import { Result } from "effect";
import { isDelta } from "@conductor/core/link";

const nextSequence = 1;

interface CursorAction {
  readonly kind: "Cursor";
  readonly cursor: Cursor;
}

interface ForwardAction {
  readonly kind: "Forward";
  readonly frame: Feed;
}

type FeedAction = CursorAction | ForwardAction;

const sameEpoch: (cursor: Cursor | undefined, epoch: Epoch) => boolean = (cursor, epoch) => cursor?.epoch === epoch;

const caughtUp: (status: Status, applied: HostSeq, liveAt: HostSeq) => boolean = (status, applied, liveAt) =>
  applied >= liveAt && status.state === "CatchingUp";

const eventActions: (welcome: Welcome, frame: EventFrame, after: HostSeq) => Result.Result<ReadonlyArray<FeedAction>, string> = (
  welcome,
  frame,
  after,
) => {
  const { seq } = frame.Event;
  const delta = isDelta(frame.Event);
  const forward: ForwardAction = { kind: "Forward", frame };
  if (seq <= after && delta) {
    return Result.succeed([forward]);
  }
  if (seq <= after) {
    return Result.succeed([]);
  }
  if (seq > after + nextSequence || delta) {
    return Result.fail(`event ${String(seq)} arrived after ${String(after)}: the feed has a gap`);
  }
  return Result.succeed([{ kind: "Cursor", cursor: { epoch: welcome.epoch, seq } }, forward]);
};

export { caughtUp, eventActions, type FeedAction, sameEpoch };
