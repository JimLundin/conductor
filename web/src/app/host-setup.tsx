import type { FunctionComponent } from "react";
import type { PairedHost } from "@conductor/core/store";
import { accountSetup } from "@conductor/core/view/app";

const title = "Accounts and checkouts";
const separator = " · ";

const HostSetup: FunctionComponent<{ readonly host: PairedHost }> = ({ host }) => {
  const checkouts = host.fold.projects.flatMap((project) =>
    project.checkouts.map((checkout) => (
      <li key={JSON.stringify([project.id, checkout.path])}>
        {project.title} <code>{checkout.path}</code>
      </li>
    )),
  );
  const accounts = accountSetup(host.fold).map((account) => (
    <li key={account.key}>
      {account.label}
      {separator}
      {account.auth}
    </li>
  ));
  return (
    <details className="host-setup">
      <summary>{title}</summary>
      <ul>
        {accounts}
        {checkouts}
      </ul>
    </details>
  );
};

export { HostSetup };
