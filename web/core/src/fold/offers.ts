import { Data, Match } from "effect";
import { Item, ToolState, isRequestOpen } from "#core/fold/transcript";
import type { Session } from "#core/fold/session";
import { SessionStatus } from "#core/fold/session";

const noItems = 0;
const lastItem = -1;

type Composer = "Open" | "Busy" | "Closed";

interface Controls {
  readonly composer: Composer;
  readonly interrupt: boolean;
  readonly continue: boolean;
  readonly finish: boolean;
}

type ListStatus = Data.TaggedEnum<{
  readonly Session: { readonly status: SessionStatus["_tag"] };
  readonly Requests: { readonly count: number };
}>;

const ListStatus = Data.taggedEnum<ListStatus>();

type EmptyTranscript = "Starting" | "Ready" | "Closed";

const openRequests = (session: Session): ReadonlyArray<Item> =>
  [...session.items, ...session.subagents.flatMap((subagent) => subagent.items)].filter((item) =>
    isRequestOpen(item),
  );

const controls = (session: Session): Controls => {
  const { _tag: status } = session.status;
  return {
    composer: Match.value(status).pipe(
      Match.withReturnType<Composer>(),
      Match.when("Idle", () => "Open"),
      Match.when("Closed", () => "Closed"),
      Match.orElse(() => "Busy"),
    ),
    continue: status === "Closed",
    finish:
      session.issue !== undefined &&
      (status === "Idle" || status === "Closed") &&
      (session.finish === "Ready" || session.finish === "Retrying"),
    interrupt: status === "Running",
  };
};

const listStatus = (session: Session): ListStatus => {
  const count = openRequests(session).length;
  const { _tag: status } = session.status;
  return count === noItems ? ListStatus.Session({ status }) : ListStatus.Requests({ count });
};

const emptyTranscript = (session: Session): EmptyTranscript | undefined => {
  if (session.items.length > noItems) {
    return undefined;
  }
  const { _tag: status } = session.status;
  return Match.value(status).pipe(
    Match.withReturnType<EmptyTranscript>(),
    Match.when("Starting", () => "Starting"),
    Match.when("Closed", () => "Closed"),
    Match.orElse(() => "Ready"),
  );
};

const isLive = (item: Item | undefined): boolean =>
  item !== undefined &&
  (((Item.$is("Reply")(item) || Item.$is("Reasoning")(item)) && item.state === "Streaming") ||
    (Item.$is("Tool")(item) && ToolState.$is("Running")(item.state)));

const isWorking = (session: Session): boolean =>
  SessionStatus.$is("Running")(session.status) &&
  openRequests(session).length === noItems &&
  !isLive(session.items.at(lastItem));

export { controls, emptyTranscript, isWorking, ListStatus, listStatus, openRequests };
