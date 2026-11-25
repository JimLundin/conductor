import type { ChangeEventHandler, FunctionComponent } from "react";
import type { Choosable, Draft } from "@conductor/core/session/start";
import { useCallback } from "react";

interface Props {
  readonly change: (next: Partial<Draft>) => void;
  readonly choices: ReadonlyArray<Choosable>;
  readonly value: string;
}

const label = "Agent";
const choose = "Choose an agent";

const AgentField: FunctionComponent<Props> = ({ change, choices, value }) => {
  const selected = useCallback<ChangeEventHandler<HTMLSelectElement>>(
    (event) => {
      change({ effort: "", instance: event.target.value, model: "" });
    },
    [change],
  );
  return (
    <label>
      {label}
      <select onChange={selected} value={value}>
        <option value="">{choose}</option>
        {choices.map((choice) => (
          <option key={choice.key} value={choice.key}>
            {choice.label}
          </option>
        ))}
      </select>
    </label>
  );
};

export { AgentField };
