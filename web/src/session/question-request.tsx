import type { Command, SessionId } from "@conductor/core/schemas/index";
import type { FunctionComponent, SubmitEventHandler } from "react";
import { answer, answersOf } from "@conductor/core/session/commands";
import { emptyPicks, questions, replacePick } from "@conductor/core/view/requests";
import { useCallback, useState } from "react";
import type { Asking } from "@conductor/core/view/requests";
import type { Picked } from "@conductor/core/session/commands";
import { QuestionField } from "#ui/session/question-field.tsx";

interface Props {
  readonly disabled: boolean;
  readonly id: SessionId;
  readonly item: Asking;
  readonly run: (body: Command) => void;
}

const answerLabel = "Answer";

const QuestionRequest: FunctionComponent<Props> = ({ disabled, id, item, run }) => {
  const [picks, setPicks] = useState(() => emptyPicks(item));
  const pick = useCallback((at: number, next: Picked): void => {
    setPicks((current) => replacePick(current, at, next));
  }, []);
  const submit = useCallback<SubmitEventHandler<HTMLFormElement>>(
    (event) => {
      event.preventDefault();
      run(answer(id, item.request, answersOf(picks)));
    },
    [id, item.request, picks, run],
  );
  return (
    <form className="request" onSubmit={submit}>
      {questions(item, picks).map((field) => (
        <QuestionField disabled={disabled} field={field} key={field.question.header} pick={pick} />
      ))}
      <div className="offers">
        <button className="primary" disabled={disabled} type="submit">
          {answerLabel}
        </button>
      </div>
    </form>
  );
};

export { QuestionRequest };
