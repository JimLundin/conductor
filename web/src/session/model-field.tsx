import type { ChangeEventHandler, FunctionComponent } from "react";
import type { Draft } from "@conductor/core/session/start";
import type { ModelChoice } from "@conductor/core/schemas/index";
import { useCallback } from "react";

interface Props {
  readonly change: (next: Partial<Draft>) => void;
  readonly choices: ReadonlyArray<ModelChoice>;
  readonly defaultLabel: string;
  readonly value: string;
}

const label = "Model";
const separator = " · ";

const ModelField: FunctionComponent<Props> = ({ change, choices, defaultLabel, value }) => {
  const selected = useCallback<ChangeEventHandler<HTMLSelectElement>>(
    (event) => {
      change({ effort: "", model: event.target.value });
    },
    [change],
  );
  return (
    <label>
      {label}
      <select onChange={selected} value={value}>
        <option value="">{defaultLabel}</option>
        {choices.map((choice) => (
          <option key={choice.id} value={choice.id}>
            {choice.name}
            {separator}
            {choice.description}
          </option>
        ))}
      </select>
    </label>
  );
};

export { ModelField };
