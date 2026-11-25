import type { FunctionComponent } from "react";
import type { Item } from "@conductor/core/fold/transcript";
import { Markdown } from "#ui/session/markdown.tsx";
import { reasoning } from "@conductor/core/view/transcript";

const Reasoning: FunctionComponent<{
  readonly item: Extract<Item, { readonly _tag: "Reasoning" }>;
}> = ({ item }) => {
  const shown = reasoning(item);
  return (
    <details className="reasoning" open={shown.open}>
      <summary>{shown.title}</summary>
      <Markdown text={item.text} />
    </details>
  );
};

export { Reasoning };
