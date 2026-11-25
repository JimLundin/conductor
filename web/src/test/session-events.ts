import { created, foldEvents, foldUpdates, received } from "#ui/test/examples.ts";
import type { EventKind } from "@conductor/core/schemas/index";
import type { Host } from "@conductor/core/fold/host";
import { Update } from "@conductor/core/link";
import { recordedFeed } from "#ui/test/recorded-feeds.ts";

const session = "00000000000000000000000001";
const directory = "/conductor/target/tmp/conductor-e2e-0-approve";
const ready = (): ReadonlyArray<EventKind> => [created(directory), "SessionReady"];

const turnThrough = (sequence: number): Host => {
  const frames = recordedFeed("a_claude_turn_streams_asks_and_finishes");
  const updates = received(frames, 0).filter(
    (update) =>
      !Update.$is("Feed")(update) ||
      !("Event" in update.frame) ||
      update.frame.Event.seq <= sequence,
  );
  return foldUpdates(updates);
};

const onSession = (kinds: ReadonlyArray<EventKind>): Host => {
  const numbered = kinds.map((kind) => [1, kind] as const);
  return foldEvents(numbered);
};

export { directory, onSession, ready, session, turnThrough };
