import { Back } from "#ui/app/back.tsx";
import { PairForm } from "#ui/app/pair-form.tsx";
import { PairedRow } from "#ui/app/paired-row.tsx";
import type { ReactElement } from "react";
import { useHosts } from "#ui/use-store.ts";

const title = "Pair a host";
const hint = "Open a host's join link on this device, or paste the join link or ticket it prints.";
const none = 0;

const Pairing = (): ReactElement => {
  const hosts = useHosts();
  return (
    <main className="pairing">
      {hosts.length === none ? undefined : <Back />}
      <h1>{title}</h1>
      <p>{hint}</p>
      <PairForm />
      {hosts.length === none ? undefined : (
        <ul className="paired-hosts">
          {hosts.map((host) => (
            <PairedRow host={host} key={host.key} />
          ))}
        </ul>
      )}
    </main>
  );
};

export { Pairing };
