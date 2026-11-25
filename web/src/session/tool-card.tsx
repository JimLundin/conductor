import type { FunctionComponent, ReactNode } from "react";
import { toolName, toolStatus } from "@conductor/core/session/words";
import { ApprovalLine } from "#ui/session/approval-line.tsx";
import type { Item } from "@conductor/core/fold/transcript";
import { ToolDetail } from "#ui/session/tool-detail.tsx";

interface Props {
  readonly children: ReactNode;
  readonly item: Extract<Item, { readonly _tag: "Tool" }>;
}

const ToolCard: FunctionComponent<Props> = ({ children, item }) => (
  <article className={`tool ${item.state._tag.toLowerCase()}`} data-status={toolStatus(item.state)}>
    <header>
      <span className="name">{toolName(item.tool)}</span>
      <span className="status">{toolStatus(item.state)}</span>
    </header>
    <ToolDetail tool={item.tool} />
    <ApprovalLine approval={item.approval} />
    {children}
  </article>
);

export { ToolCard };
