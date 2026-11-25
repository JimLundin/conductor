import type { Epoch, Event, Feed, HostSeq, Welcome } from "#core/schemas/index";
import { Data } from "effect";
import type { Unit } from "#core/data";

type LinkState = "Disconnected" | "Connecting" | "Authenticating" | "CatchingUp" | "Live";
interface Status {
  readonly state: LinkState;
  readonly error: string | undefined;
}
type Update = Data.TaggedEnum<{
  readonly Status: Status;
  readonly Reset: { readonly epoch: Epoch };
  readonly Welcome: { readonly welcome: Welcome };
  readonly Feed: { readonly frame: Feed };
}>;
const Update = Data.taggedEnum<Update>();
interface Cursor {
  readonly epoch: Epoch;
  readonly seq: HostSeq;
}
type EventFrame = Extract<Feed, { readonly Event: unknown }>;
type SendError = Data.TaggedEnum<{
  readonly NotConnected: Unit;
  readonly NoResult: Unit;
  readonly Wire: { readonly reason: string };
}>;
const SendError = Data.taggedEnum<SendError>();

// A live delta repeats the last stored seq, which may be another session's.
const isDelta = (event: Event): boolean =>
  typeof event.kind === "object" &&
  "Agent" in event.kind &&
  ("AssistantDelta" in event.kind.Agent || "ReasoningDelta" in event.kind.Agent);

export { type Cursor, type EventFrame, isDelta, type LinkState, SendError, type Status, Update };
