import { AgentField } from "#ui/session/agent-field.tsx";
import { DirectoryField } from "#ui/session/directory-field.tsx";
import type { Draft } from "@conductor/core/session/start";
import type { FunctionComponent } from "react";
import type { Host } from "@conductor/core/fold/host";
import { ModelFields } from "#ui/session/model-fields.tsx";
import { ProjectField } from "#ui/session/project-field.tsx";
import { fields } from "@conductor/core/view/start";

interface Props {
  readonly change: (next: Partial<Draft>) => void;
  readonly draft: Draft;
  readonly host: Host | undefined;
}

const InstanceFields: FunctionComponent<Props> = ({ change, draft, host }) => {
  const shown = host === undefined ? undefined : fields(host, draft);
  return shown === undefined ? undefined : (
    <>
      <AgentField change={change} choices={shown.instances} value={draft.instance} />
      <ProjectField change={change} choices={shown.projects} value={draft.project} />
      <DirectoryField
        change={change}
        shown={shown.directory}
        suggested={shown.suggested}
        value={draft.directory}
      />
      <ModelFields change={change} draft={draft} fields={shown} />
    </>
  );
};

export { InstanceFields };
