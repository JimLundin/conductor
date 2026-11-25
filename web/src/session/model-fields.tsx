import type { Draft } from "@conductor/core/session/start";
import { EffortField } from "#ui/session/effort-field.tsx";
import type { Fields } from "@conductor/core/view/start";
import type { FunctionComponent } from "react";
import { ModelField } from "#ui/session/model-field.tsx";

interface Props {
  readonly change: (next: Partial<Draft>) => void;
  readonly draft: Draft;
  readonly fields: Fields;
}

const ModelFields: FunctionComponent<Props> = ({ change, draft, fields: shown }) =>
  shown.models === undefined ? undefined : (
    <>
      <ModelField
        change={change}
        choices={shown.models.choices}
        defaultLabel={shown.modelDefault}
        value={draft.model}
      />
      <EffortField change={change} choices={shown.efforts} value={draft.effort} />
    </>
  );

export { ModelFields };
