import type { Command } from "@conductor/core/schemas/index";
import { Effect } from "effect";
import type { Host } from "@conductor/core/fold/host";
import type { HostLink } from "#ui/link/host.ts";
import type { Offer } from "@conductor/core/session/commands";
import { SessionView } from "#ui/session/session-view.tsx";
import { expect } from "@effect/vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { session } from "#ui/test/session-events.ts";
import { sessionOf } from "#ui/test/examples.ts";
import { sessionOffers } from "@conductor/core/session/commands";

interface Recording {
  readonly send: HostLink["send"];
  readonly sent: () => ReadonlyArray<Command>;
}

interface ShownSession {
  readonly choices: ReadonlyArray<Offer>;
  readonly html: string;
}

const recording = (): Recording => {
  let sent: ReadonlyArray<Command> = [];
  return {
    send: (body) =>
      Effect.sync(() => {
        sent = [...sent, body];
        return { Session: "Accepted" } as const;
      }),
    sent: () => sent,
  };
};

const view = (host: Host): string => {
  const { send, sent } = recording();
  const html = renderToStaticMarkup(
    <SessionView
      hostName="test-host"
      id={session}
      send={send}
      session={sessionOf(host, session)}
    />,
  );
  expect(sent()).toStrictEqual([]);
  return html;
};

const button = (label: string): string => `type="button">${label}</button>`;

const shownSession = (host: Host): ShownSession => {
  const current = sessionOf(host, session);
  return { choices: sessionOffers(session, current), html: view(host) };
};

export { button, recording, shownSession, view };
