import type { FunctionComponent } from "react";
import type { Item } from "@conductor/core/fold/transcript";
import { ItemRow } from "#ui/session/item-row.tsx";
import type { Subagent } from "@conductor/core/fold/subagent";
import { SubagentCard } from "#ui/session/subagent-card.tsx";
import { rows } from "@conductor/core/view/transcript";

interface Props {
  readonly items: ReadonlyArray<Item>;
  readonly subagents: ReadonlyArray<Subagent>;
}
const noChildren: ReadonlyArray<Subagent> = [];

const Items: FunctionComponent<Props> = ({ items, subagents }) => (
  <ol className="items">
    {rows(items, subagents).map((row) => {
      const nested = row.children.map((subagent) => (
        <SubagentCard key={subagent.id} subagent={subagent}>
          <Items items={subagent.items} subagents={noChildren} />
        </SubagentCard>
      ));
      return (
        <li className={`item ${row.style}`} key={row.key}>
          <ItemRow item={row.item}>{nested}</ItemRow>
        </li>
      );
    })}
  </ol>
);

export { Items };
