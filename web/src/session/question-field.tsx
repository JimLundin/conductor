import type { ChangeEventHandler, FunctionComponent } from "react";
import type { Picked } from "@conductor/core/session/commands";
import { QuestionOption } from "#ui/session/question-option.tsx";
import type { questions } from "@conductor/core/view/requests";
import { useCallback } from "react";

interface Props {
  readonly disabled: boolean;
  readonly field: ReturnType<typeof questions>[number];
  readonly pick: (at: number, next: Picked) => void;
}

const QuestionField: FunctionComponent<Props> = ({ disabled, field, pick }) => {
  const { index, picked, question } = field;
  const typed = useCallback<ChangeEventHandler<HTMLInputElement>>(
    (event) => {
      pick(index, { ...picked, typed: event.target.value });
    },
    [index, pick, picked],
  );
  return (
    <fieldset>
      <legend>{question.header}</legend>
      <p>{question.text}</p>
      <div className="options">
        {question.options.map((option) => (
          <QuestionOption
            disabled={disabled}
            field={field}
            key={option.label}
            option={option}
            pick={pick}
          />
        ))}
      </div>
      <input
        aria-label={`Another answer to ${question.header}`}
        disabled={disabled}
        onChange={typed}
        placeholder="Or type an answer"
        value={picked.typed}
      />
    </fieldset>
  );
};

export { QuestionField };
