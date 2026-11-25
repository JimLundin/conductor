import type { FunctionComponent } from "react";
import type { Item } from "@conductor/core/fold/transcript";
import { Markdown } from "#ui/session/markdown.tsx";

const Reply: FunctionComponent<{
  readonly item: Extract<Item, { readonly _tag: "Reply" }>;
}> = ({ item }) => (
  <div className={`reply ${item.state.toLowerCase()}`}>
    <Markdown text={item.text} />
  </div>
);

export { Reply };
