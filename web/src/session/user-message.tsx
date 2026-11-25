import type { FunctionComponent } from "react";
import type { Item } from "@conductor/core/fold/transcript";

const UserMessage: FunctionComponent<{
  readonly item: Extract<Item, { readonly _tag: "User" }>;
}> = ({ item }) => (
  <div className="user">
    <p className="bubble">{item.text}</p>
    <span className="by">{item.by}</span>
  </div>
);

export { UserMessage };
