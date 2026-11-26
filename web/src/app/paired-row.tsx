import { HostOperation, operationText } from "@conductor/core/pairing";
import type { FunctionComponent } from "react";
import { HostSetup } from "#ui/app/host-setup.tsx";
import { LinkStatus } from "#ui/app/link-status.tsx";
import { Notice } from "#ui/session/notice.tsx";
import type { PairedHost } from "@conductor/core/store";
import { nameOf } from "@conductor/core/store";
import { useCallback } from "react";
import { useStore } from "#ui/use-store.ts";

const forget = "Forget";
const retryLabel = "Retry";

const PairedRow: FunctionComponent<{ readonly host: PairedHost }> = ({ host }) => {
  const store = useStore();
  const remove = useCallback((): void => {
    store.forget(host.key);
  }, [host.key, store]);
  const retry = useCallback((): void => {
    store.retry(host.key);
  }, [host.key, store]);
  const pending = HostOperation.$is("Pending")(host.operation);
  const failed = HostOperation.$is("Failed")(host.operation);
  return (
    <li className="paired" data-host={host.key}>
      <span className="paired-name">{nameOf(host)}</span>
      <LinkStatus status={host.status} />
      <Notice
        tone={failed ? "error" : "notice"}
        role={failed ? "alert" : "status"}
        text={operationText(host.operation)}
      />
      {failed ? (
        <button onClick={retry} type="button">
          {retryLabel}
        </button>
      ) : undefined}
      <button className="forget" disabled={pending} onClick={remove} type="button">
        {forget}
      </button>
      <HostSetup host={host} />
    </li>
  );
};

export { PairedRow };
