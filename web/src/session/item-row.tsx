import type { FunctionComponent, ReactElement, ReactNode } from "react";
import { AnsweredQuestion } from "#ui/session/answered-question.tsx";
import { Item } from "@conductor/core/fold/transcript";
import { Marker } from "#ui/session/marker.tsx";
import { Reasoning } from "#ui/session/reasoning.tsx";
import { Reply } from "#ui/session/reply.tsx";
import { ToolCard } from "#ui/session/tool-card.tsx";
import { UserMessage } from "#ui/session/user-message.tsx";

interface Props {
  readonly children: ReactNode;
  readonly item: Item;
}
type Shown = (children: ReactNode) => ReactElement;

const shown: (item: Item) => Shown = Item.$match({
  Continued: (item) => (): ReactElement => <Marker item={item} />,
  Error: (item) => (): ReactElement => <Marker item={item} />,
  Finished: (item) => (): ReactElement => <Marker item={item} />,
  Finishing: (item) => (): ReactElement => <Marker item={item} />,
  ModelChanged: (item) => (): ReactElement => <Marker item={item} />,
  Question: (item) => (): ReactElement => <AnsweredQuestion item={item} />,
  Reasoning: (item) => (): ReactElement => <Reasoning item={item} />,
  Reply: (item) => (): ReactElement => <Reply item={item} />,
  Tool:
    (item) =>
    (children: ReactNode): ReactElement => <ToolCard item={item}>{children}</ToolCard>,
  TurnCompleted: (item) => (): ReactElement => <Marker item={item} />,
  User: (item) => (): ReactElement => <UserMessage item={item} />,
});

const ItemRow: FunctionComponent<Props> = ({ children, item }) => shown(item)(children);

export { ItemRow };
