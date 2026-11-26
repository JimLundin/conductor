import { pairingHref, workHref } from "@conductor/core/view/app";
import { HostGroup } from "#ui/app/host-group.tsx";
import { ProjectList } from "#ui/app/project-list.tsx";
import type { ReactElement } from "react";
import { useHosts } from "#ui/use-store.ts";

const title = "Conductor";
const hosts = "Hosts";
const start = "New session";

const Sidebar = (): ReactElement => (
  <nav aria-label="Sessions" className="sidebar">
    <header className="band">
      <span className="band-title">{title}</span>
      <a href={workHref()}>{start}</a>
      <a className="add-host" href={pairingHref}>
        {hosts}
      </a>
    </header>
    <ProjectList />
    {useHosts().map((host) => (
      <HostGroup host={host} key={host.key} />
    ))}
  </nav>
);

export { Sidebar };
