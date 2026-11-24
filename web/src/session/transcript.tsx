// A session's transcript: its folded items in order, each tool call as a card with the
// sub-agents it started under it.
import type { ReactElement } from "react";
import type { Subagent } from "../fold/subagent.ts";
import { type Approval, Item } from "../fold/transcript.ts";
import type { ToolKind } from "../schemas/index.ts";
import { Markdown } from "./markdown.tsx";
import {
  closeText,
  finishedText,
  outcomeText,
  requestText,
  subagentStatus,
  toolName,
  toolStatus,
} from "./words.ts";

type Tool = Extract<Item, { readonly _tag: "Tool" }>;

const ToolDetail = ({ tool }: { readonly tool: ToolKind }): ReactElement => {
  if ("Command" in tool) {
    return <pre className="command">{tool.Command.command}</pre>;
  }
  if ("FileChange" in tool) {
    return (
      <ul className="files">
        {tool.FileChange.files.map((file) => (
          <li key={`${file.change} ${file.path}`}>
            <span className={`change ${file.change.toLowerCase()}`}>{file.change}</span>
            <code>{file.path}</code>
          </li>
        ))}
      </ul>
    );
  }
  return <pre className="command">{tool.Other.detail}</pre>;
};

const ApprovalLine = ({ approval }: { readonly approval: Approval }): ReactElement | null =>
  approval._tag === "Asked" ? (
    <p className={`approval ${approval.state._tag.toLowerCase()}`}>
      {requestText(approval.state, "Waiting for approval")}
      {approval.reason === null ? null : <span className="reason"> · {approval.reason}</span>}
    </p>
  ) : null;

const SubagentCard = ({ subagent }: { readonly subagent: Subagent }): ReactElement => (
  <details className="subagent" open={subagent.status._tag === "Running"}>
    <summary>
      <span className="title">{subagent.description}</span>
      {subagent.kind === null ? null : <span className="kind">{subagent.kind}</span>}
      <span className="status">{subagentStatus(subagent)}</span>
    </summary>
    <Items items={subagent.items} subagents={[]} />
  </details>
);

const ToolCard = ({
  item,
  subagents,
}: {
  readonly item: Tool;
  readonly subagents: readonly Subagent[];
}): ReactElement => (
  <article className={`tool ${item.state._tag.toLowerCase()}`} data-status={toolStatus(item.state)}>
    <header>
      <span className="name">{toolName(item.tool)}</span>
      <span className="status">{toolStatus(item.state)}</span>
    </header>
    <ToolDetail tool={item.tool} />
    <ApprovalLine approval={item.approval} />
    {subagents
      .filter((subagent) => subagent.call === item.call)
      .map((subagent) => (
        <SubagentCard key={subagent.id} subagent={subagent} />
      ))}
  </article>
);

const Marker = ({ tone, text }: { readonly tone: string; readonly text: string }): ReactElement => (
  <p className={`marker ${tone}`}>{text}</p>
);

type Shown = (subagents: readonly Subagent[]) => ReactElement;

// Each item as the transcript shows it; only a tool card needs the sub-agents, to show those
// its call started under it.
const itemView: (item: Item) => Shown = Item.$match({
  User:
    ({ text, by }) =>
    (): ReactElement => (
      <div className="user">
        <p className="bubble">{text}</p>
        <span className="by">{by}</span>
      </div>
    ),
  Reply:
    ({ text, state }) =>
    (): ReactElement => (
      <div className={`reply ${state.toLowerCase()}`}>
        <Markdown text={text} />
      </div>
    ),
  Reasoning:
    ({ text, state }) =>
    (): ReactElement => (
      <details className="reasoning" open={state === "Streaming"}>
        <summary>{state === "Streaming" ? "Thinking" : "Thought"}</summary>
        <Markdown text={text} />
      </details>
    ),
  Tool:
    (tool) =>
    (subagents: readonly Subagent[]): ReactElement => (
      <ToolCard item={tool} subagents={subagents} />
    ),
  Question:
    ({ questions, state }) =>
    (): ReactElement => (
      <article className={`question ${state._tag.toLowerCase()}`}>
        {questions.map((question) => (
          <p key={question.header}>
            <strong>{question.header}</strong> {question.text}
          </p>
        ))}
        <p className="approval">{requestText(state, "Waiting for your answer")}</p>
      </article>
    ),
  TurnCompleted:
    ({ outcome }) =>
    (): ReactElement => (
      <Marker tone={outcome === "Completed" ? "done" : "stopped"} text={outcomeText(outcome)} />
    ),
  Continued:
    ({ reason }) =>
    (): ReactElement => <Marker tone="done" text={`Continued after it ${closeText(reason)}`} />,
  ModelChanged:
    ({ reason }) =>
    (): ReactElement => <Marker tone="note" text={`Model changed: ${reason}`} />,
  Finishing: () => (): ReactElement => <Marker tone="note" text="Finishing on its issue" />,
  Finished:
    ({ finished }) =>
    (): ReactElement => <Marker tone="done" text={finishedText(finished)} />,
  Error:
    ({ message }) =>
    (): ReactElement => <Marker tone="error" text={message} />,
});

// Items have no identity of their own: the transcript only appends and changes them in place,
// so an item's position is its identity.
const Items = ({
  items,
  subagents,
}: {
  readonly items: readonly Item[];
  readonly subagents: readonly Subagent[];
}): ReactElement => (
  <ol className="items">
    {items
      .map((item, at) => ({ item, key: `${at}` }))
      .map(({ item, key }) => (
        <li key={key} className={`item ${item._tag.toLowerCase()}`}>
          {itemView(item)(subagents)}
        </li>
      ))}
  </ol>
);

export { Items, ToolDetail };
