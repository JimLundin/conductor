import type { ChangeEventHandler, FunctionComponent } from "react";
import type { ValueChange } from "#ui/event.ts";
import { useCallback } from "react";

interface Props {
  readonly change: (host: string) => void;
  readonly hosts: ReadonlyArray<{ readonly key: string; readonly name: string }>;
  readonly value: string;
}

const label = "Host";
const choose = "Choose a host";

const HostField: FunctionComponent<Props> = ({ change, hosts, value }) => {
  const selected = useCallback<ChangeEventHandler<HTMLSelectElement>>(
    (event: ValueChange) => {
      change(event.target.value);
    },
    [change],
  );
  return (
    <label>
      {label}
      <select onChange={selected} value={value}>
        <option value="">{choose}</option>
        {hosts.map((host) => (
          <option key={host.key} value={host.key}>
            {host.name}
          </option>
        ))}
      </select>
    </label>
  );
};

export { HostField };
