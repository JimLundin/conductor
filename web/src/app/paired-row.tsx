import type { FunctionComponent } from "react";
import { LinkStatus } from "#ui/app/link-status.tsx";
import type { PairedHost } from "@conductor/core/store";
import { nameOf } from "@conductor/core/store";
import { useCallback } from "react";
import { useStore } from "#ui/use-store.ts";

const forget = "Forget";

const PairedRow: FunctionComponent<{ readonly host: PairedHost }> = ({ host }) => {
  const store = useStore();
  const remove = useCallback((): void => {
    store.forget(host.key);
  }, [host.key, store]);
  return (
    <li className="paired" data-host={host.key}>
      <span className="paired-name">{nameOf(host)}</span>
      <LinkStatus status={host.status} />
      <button className="forget" onClick={remove} type="button">
        {forget}
      </button>
    </li>
  );
};

export { PairedRow };
