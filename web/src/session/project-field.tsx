import type { ChangeEventHandler, FunctionComponent } from "react";
import type { Draft } from "@conductor/core/session/start";
import type { ValueChange } from "#ui/event.ts";
import { useCallback } from "react";

interface Props {
  readonly change: (next: Partial<Draft>) => void;
  readonly choices: ReadonlyArray<{ readonly id: string; readonly title: string }>;
  readonly value: string;
}

const label = "Project";
const directory = "Git directory";

const ProjectField: FunctionComponent<Props> = ({ change, choices, value }) => {
  const selected = useCallback<ChangeEventHandler<HTMLSelectElement>>(
    (event: ValueChange) => {
      change({ agent: "", effort: "", issue: "", model: "", project: event.target.value });
    },
    [change],
  );
  return (
    <label>
      {label}
      <select onChange={selected} value={value}>
        <option value="">{directory}</option>
        {value !== "" && !choices.some((one) => one.id === value) ? (
          <option value={value}>{value}</option>
        ) : undefined}
        {choices.map((choice) => (
          <option key={choice.id} value={choice.id}>
            {choice.title}
          </option>
        ))}
      </select>
    </label>
  );
};

export { ProjectField };
