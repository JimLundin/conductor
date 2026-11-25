import type { ChangeEventHandler, FunctionComponent } from "react";
import type { Draft } from "@conductor/core/session/start";
import { useCallback } from "react";

interface Props {
  readonly change: (next: Partial<Draft>) => void;
  readonly choices: ReadonlyArray<string>;
  readonly value: string;
}

const label = "Effort";
const defaultEffort = "The model's default";

const EffortField: FunctionComponent<Props> = ({ change, choices, value }) => {
  const selected = useCallback<ChangeEventHandler<HTMLSelectElement>>(
    (event) => {
      change({ effort: event.target.value });
    },
    [change],
  );
  return (
    <label>
      {label}
      <select onChange={selected} value={value}>
        <option value="">{defaultEffort}</option>
        {choices.map((choice) => (
          <option key={choice} value={choice}>
            {choice}
          </option>
        ))}
      </select>
    </label>
  );
};

export { EffortField };
