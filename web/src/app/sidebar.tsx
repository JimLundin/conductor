import { HostGroup } from "#ui/app/host-group.tsx";
import type { ReactElement } from "react";
import { pairingHref } from "@conductor/core/view/app";
import { useHosts } from "#ui/use-store.ts";

const title = "Conductor";
const hosts = "Hosts";

const Sidebar = (): ReactElement => (
  <nav aria-label="Sessions" className="sidebar">
    <header className="band">
      <span className="band-title">{title}</span>
      <a className="add-host" href={pairingHref}>
        {hosts}
      </a>
    </header>
    {useHosts().map((host) => (
      <HostGroup host={host} key={host.key} />
    ))}
  </nav>
);

export { Sidebar };
