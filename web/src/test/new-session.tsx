import type { ComponentProps } from "react";
import type { Host } from "@conductor/core/fold/host";
import { NewSession } from "#ui/session/new-session.tsx";
import { recording } from "#ui/test/session.tsx";
import { renderToStaticMarkup } from "react-dom/server";

const started = (): void => undefined;

const availableHosts = (host: Host): ComponentProps<typeof NewSession>["hosts"] => [
  { host, key: "laptop", name: "test-host", send: recording().send },
];

const newSessionForm = (host: Host): string => {
  const hosts = availableHosts(host);
  return renderToStaticMarkup(<NewSession hosts={hosts} initial="laptop" started={started} />);
};

export { newSessionForm };
