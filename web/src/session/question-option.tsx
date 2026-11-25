import type { FunctionComponent } from "react";
import type { Picked } from "@conductor/core/session/commands";
import type { questions } from "@conductor/core/view/requests";
import { toggled } from "@conductor/core/session/commands";
import { useCallback } from "react";

type Field = ReturnType<typeof questions>[number];

interface Props {
  readonly disabled: boolean;
  readonly field: Field;
  readonly option: Field["question"]["options"][number];
  readonly pick: (at: number, next: Picked) => void;
}

const QuestionOption: FunctionComponent<Props> = ({ disabled, field, option, pick }) => {
  const { index, picked, question } = field;
  const click = useCallback((): void => {
    pick(index, toggled(question, picked, option.label));
  }, [index, option.label, pick, picked, question]);
  return (
    <button
      aria-pressed={picked.labels.includes(option.label)}
      disabled={disabled}
      onClick={click}
      title={option.description}
      type="button"
    >
      {option.label}
    </button>
  );
};

export { QuestionOption };
