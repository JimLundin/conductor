import type { Issue, IssueId, Project, ProjectId } from "#core/schemas/index";
import type { Host } from "#core/fold/host";
import type { PairedHost } from "#core/store";

const none = 0;

const projectsOn = (hosts: ReadonlyArray<PairedHost>): ReadonlyArray<Project> =>
  hosts.flatMap((host) => host.fold.projects);

const projects = (hosts: ReadonlyArray<PairedHost>): ReadonlyArray<Project> => [
  ...new Map(projectsOn(hosts).map((project) => [project.id, project])).values(),
];

const validIssues = (project: Project): ReadonlyArray<Issue> =>
  project.issues.flatMap(({ state }) => ("Valid" in state ? [state.Valid.issue] : []));

const issues = (hosts: ReadonlyArray<PairedHost>, project: ProjectId): ReadonlyArray<Issue> => [
  ...new Map(
    projectsOn(hosts)
      .filter((found) => found.id === project)
      .flatMap((found) => validIssues(found))
      .map((issue) => [issue.id, issue]),
  ).values(),
];

const hasCheckout = (host: Host, project: ProjectId, issue: IssueId): boolean =>
  host.projects.some(
    (found) =>
      found.id === project &&
      found.checkouts.length > none &&
      (issue === "" || validIssues(found).some((record) => record.id === issue)),
  );

export { hasCheckout, issues, projects };
