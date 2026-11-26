import type { Choosable, Draft } from "@conductor/core/session/start";
import { AgentField } from "#ui/session/agent-field.tsx";
import type { FunctionComponent } from "react";
import { ModelFields } from "#ui/session/model-fields.tsx";
import { fields } from "@conductor/core/view/start";

interface Props {
  readonly change: (next: Partial<Draft>) => void;
  readonly draft: Draft;
  readonly options: ReadonlyArray<Choosable>;
  readonly chosen: Choosable | undefined;
}

const none = 0;
const one = 1;
const unavailable = "No connected host has an available agent for this work.";
const settings = "Model and effort";

const InstanceFields: FunctionComponent<Props> = ({ change, draft, options, chosen }) => (
  <>
    {options.length === none ? <output>{unavailable}</output> : undefined}
    {options.length > one || (options.length === one && chosen === undefined) ? (
      <AgentField change={change} choices={options} value={chosen?.key ?? ""} />
    ) : undefined}
    {chosen === undefined ? undefined : (
      <details>
        <summary>{settings}</summary>
        <ModelFields change={change} draft={draft} fields={fields(chosen, draft)} />
      </details>
    )}
  </>
);

export { InstanceFields };
