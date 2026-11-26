import type { ChangeEventHandler, FunctionComponent } from "react";
import type { Choosable, Draft } from "@conductor/core/session/start";
import { issues, projects } from "@conductor/core/session/work";
import { DirectoryField } from "#ui/session/directory-field.tsx";
import type { PairedHost } from "@conductor/core/store";
import { ProjectField } from "#ui/session/project-field.tsx";
import type { ValueChange } from "#ui/event.ts";
import { suggestions } from "@conductor/core/view/start";
import { useCallback } from "react";

interface Props {
  readonly change: (next: Partial<Draft>) => void;
  readonly draft: Draft;
  readonly chosen: Choosable | undefined;
  readonly hosts: ReadonlyArray<PairedHost>;
}

const issueLabel = "Issue";
const withoutIssue = "Work without an issue";
const none = 0;

const WorkFields: FunctionComponent<Props> = ({ change, chosen, draft, hosts }) => {
  const options = issues(hosts, draft.project);
  const issue = options.find((one) => one.id === draft.issue);
  const selectIssue = useCallback<ChangeEventHandler<HTMLSelectElement>>(
    (event: ValueChange) => {
      change({ agent: "", effort: "", issue: event.target.value, model: "" });
    },
    [change],
  );
  const issueField =
    draft.project !== "" && (options.length > none || draft.issue !== "") ? (
      <label>
        {issueLabel}
        <select onChange={selectIssue} value={draft.issue}>
          <option value="">{withoutIssue}</option>
          {draft.issue !== "" && issue === undefined ? (
            <option value={draft.issue}>{draft.issue}</option>
          ) : undefined}
          {options.map((one) => (
            <option key={one.id} value={one.id}>
              {one.title}
            </option>
          ))}
        </select>
      </label>
    ) : undefined;
  return (
    <>
      <ProjectField change={change} choices={projects(hosts)} value={draft.project} />
      {issueField}
      {issue !== undefined && issue.body !== "" ? (
        <p className="issue-brief">{issue.body}</p>
      ) : undefined}
      <DirectoryField
        change={change}
        shown={draft.project === ""}
        suggested={suggestions(chosen)}
        value={draft.directory}
      />
    </>
  );
};

export { WorkFields };
