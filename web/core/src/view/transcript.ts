import { closeText, finishedText, outcomeText } from "@conductor/core/session/words";
import { Item } from "@conductor/core/fold/transcript";
import { Match } from "effect";
import type { Subagent } from "@conductor/core/fold/subagent";

type MarkerItem = Extract<
  Item,
  {
    readonly _tag:
      | "TurnCompleted"
      | "Continued"
      | "ModelChanged"
      | "Finishing"
      | "Finished"
      | "Error";
  }
>;
interface Marker {
  readonly text: string;
  readonly tone: string;
}
interface Reasoning {
  readonly open: boolean;
  readonly title: string;
}
interface Row {
  readonly children: ReadonlyArray<Subagent>;
  readonly item: Item;
  readonly key: string;
  readonly style: string;
}

const marker = (item: MarkerItem): Marker =>
  Match.value(item).pipe(
    Match.tag("Continued", ({ reason }) => ({
      text: `Continued after it ${closeText(reason)}`,
      tone: "done",
    })),
    Match.tag("Error", ({ message }) => ({ text: message, tone: "error" })),
    Match.tag("Finished", ({ finished }) => ({ text: finishedText(finished), tone: "done" })),
    Match.tag("Finishing", () => ({ text: "Finishing on its issue", tone: "note" })),
    Match.tag("ModelChanged", ({ reason }) => ({ text: `Model changed: ${reason}`, tone: "note" })),
    Match.tag("TurnCompleted", ({ outcome }) => ({
      text: outcomeText(outcome),
      tone: outcome === "Completed" ? "done" : "stopped",
    })),
    Match.exhaustive,
  );

const children = (item: Item, subagents: ReadonlyArray<Subagent>): ReadonlyArray<Subagent> =>
  Item.$is("Tool")(item) ? subagents.filter((subagent) => subagent.call === item.call) : [];

// Transcript positions remain stable while their messages stream.
const rows = (items: ReadonlyArray<Item>, subagents: ReadonlyArray<Subagent>): ReadonlyArray<Row> =>
  items.map((item, index) => ({
    children: children(item, subagents),
    item,
    key: String(index),
    style: item._tag.toLowerCase(),
  }));

const reasoning = (item: Extract<Item, { readonly _tag: "Reasoning" }>): Reasoning => ({
  open: item.state === "Streaming",
  title: item.state === "Streaming" ? "Thinking" : "Thought",
});

export { marker, reasoning, rows };
export type { MarkerItem };
