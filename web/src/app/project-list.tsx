import type { Issue, Project } from "@conductor/core/schemas/index";
import { issues, projects } from "@conductor/core/session/work";
import type { ReactElement } from "react";
import { useHosts } from "#ui/use-store.ts";
import { workHref } from "@conductor/core/view/app";

const title = "Projects";
const start = "Start in project";
const none = 0;

const projectWork = (project: Project, records: ReadonlyArray<Issue>): ReactElement => {
  const links = records.map((issue) => (
    <li key={issue.id}>
      <a href={workHref(project.id, issue.id)}>{issue.title}</a>
    </li>
  ));
  const projectLink = <a href={workHref(project.id)}>{start}</a>;
  return (
    <details key={project.id}>
      <summary>{project.title}</summary>
      <ul>
        <li>{projectLink}</li>
        {links}
      </ul>
    </details>
  );
};

const ProjectList = (): ReactElement | undefined => {
  const hosts = useHosts();
  const listed = projects(hosts);
  return listed.length === none ? undefined : (
    <section className="projects">
      <h2>{title}</h2>
      {listed.map((project) => projectWork(project, issues(hosts, project.id)))}
    </section>
  );
};

export { ProjectList };
