import type { FunctionComponent } from "react";
import type { Item } from "@conductor/core/fold/transcript";
import { requestText } from "@conductor/core/session/words";

const separator = " ";

const AnsweredQuestion: FunctionComponent<{
  readonly item: Extract<Item, { readonly _tag: "Question" }>;
}> = ({ item }) => (
  <article className={`question ${item.state._tag.toLowerCase()}`}>
    {item.questions.map((question) => (
      <p key={question.header}>
        <strong>{question.header}</strong>
        {separator}
        {question.text}
      </p>
    ))}
    <p className="approval">{requestText(item.state, "Waiting for your answer")}</p>
  </article>
);

export { AnsweredQuestion };
