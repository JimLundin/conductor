import { useHosts, useStore, useView } from "#ui/use-store.ts";
import { Back } from "#ui/app/back.tsx";
import { Notice } from "#ui/session/notice.tsx";
import { PairForm } from "#ui/app/pair-form.tsx";
import { PairedRow } from "#ui/app/paired-row.tsx";
import type { ReactElement } from "react";
import { useCallback } from "react";

const title = "Pair a host";
const hint = "Open a host's join link on this device, or paste the join link or ticket it prints.";
const none = 0;
const retryLabel = "Retry reading saved hosts";

const Pairing = (): ReactElement => {
  const hosts = useHosts();
  const store = useStore();
  const { restoration } = useView();
  const retry = useCallback((): void => {
    store.restore();
  }, [store]);
  return (
    <main className="pairing">
      {hosts.length === none ? undefined : <Back />}
      <h1>{title}</h1>
      <p>{hint}</p>
      <PairForm />
      {restoration === "Failed" ? (
        <div>
          <Notice tone="error" role="alert" text="Saved hosts could not be read." />
          <button onClick={retry} type="button">
            {retryLabel}
          </button>
        </div>
      ) : undefined}
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
