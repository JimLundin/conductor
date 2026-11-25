import type { ComponentProps, ReactElement } from "react";
import { useCallback, useId } from "react";
import { useStore, useView } from "#ui/use-store.ts";
import { Notice } from "#ui/session/notice.tsx";
import { pairingNotice } from "@conductor/core/view/app";

type FormAction = Exclude<ComponentProps<"form">["action"], string | undefined>;

const label = "Join link or ticket";
const submit = "Pair";
const rows = 3;

const PairForm = (): ReactElement => {
  const store = useStore();
  const { notice } = useView();
  const field = useId();
  const pair = useCallback<FormAction>(
    (data: Readonly<Pick<FormData, "get">>) => {
      const ticket = data.get("ticket");
      store.pair(typeof ticket === "string" ? ticket : "");
    },
    [store],
  );
  const shown = pairingNotice(notice);
  return (
    <form action={pair} className="pair-form">
      <label htmlFor={field}>{label}</label>
      <textarea
        autoCapitalize="off"
        autoComplete="off"
        id={field}
        name="ticket"
        required
        rows={rows}
        spellCheck={false}
      />
      <Notice tone="error" role="alert" text={shown} />
      <button type="submit">{submit}</button>
    </form>
  );
};

export { PairForm };
