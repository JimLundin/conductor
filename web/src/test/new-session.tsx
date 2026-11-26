import { HostAction, HostOperation } from "@conductor/core/pairing";
import type { ComponentProps } from "react";
import type { Host } from "@conductor/core/fold/host";
import { NewSession } from "#ui/session/new-session.tsx";
import type { PairedHost } from "@conductor/core/store";
import { recording } from "#ui/test/session.tsx";
import { renderToStaticMarkup } from "react-dom/server";
import { ticket } from "#ui/test/replay.ts";

const started = (): void => undefined;

const pairedHost = (key: string, fold: Host): PairedHost => ({
  fold: { ...fold, name: key },
  key,
  operation: HostOperation.Complete({ action: HostAction.Restore() }),
  saved: undefined,
  status: { error: undefined, state: "Live" },
  ticket,
});

const send: ComponentProps<typeof NewSession>["send"] = (_host, command) =>
  recording().send(command);

const newSessionForm = (
  props: Omit<ComponentProps<typeof NewSession>, "send" | "started">,
): string =>
  renderToStaticMarkup(
    <NewSession
      hosts={props.hosts}
      issue={props.issue}
      project={props.project}
      directoryHost={props.directoryHost}
      send={send}
      started={started}
    />,
  );

export { newSessionForm, pairedHost };
