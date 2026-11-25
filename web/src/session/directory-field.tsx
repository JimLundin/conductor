import type { ChangeEventHandler, FunctionComponent } from "react";
import { useCallback, useId } from "react";
import type { Draft } from "@conductor/core/session/start";
import type { ValueChange } from "#ui/event.ts";

interface Props {
  readonly change: (next: Partial<Draft>) => void;
  readonly shown: boolean;
  readonly suggested: ReadonlyArray<string>;
  readonly value: string;
}

const label = "Directory";

const DirectoryField: FunctionComponent<Props> = ({ change, shown, suggested, value }) => {
  const listed = useId();
  const selected = useCallback<ChangeEventHandler<HTMLInputElement>>(
    (event: ValueChange) => {
      change({ directory: event.target.value });
    },
    [change],
  );
  return shown ? (
    <label>
      {label}
      <input
        list={listed}
        onChange={selected}
        placeholder="A directory on the host"
        value={value}
      />
      <datalist id={listed}>
        {suggested.map((path) => (
          <option aria-label={path} key={path} value={path} />
        ))}
      </datalist>
    </label>
  ) : undefined;
};

export { DirectoryField };
