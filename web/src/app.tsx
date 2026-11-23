import type { ReactElement } from "react";
import { Route } from "./route.ts";

const pairing = {
	title: "Pair a host",
	hint: "Open a host's join link on this device, or paste the ticket it prints.",
} as const;

const Pairing = (): ReactElement => (
	<main className="pairing">
		<h1>{pairing.title}</h1>
		<p>{pairing.hint}</p>
	</main>
);

const screen = Route.$match({ Pairing });

export const App = ({ route }: { readonly route: Route }): ReactElement =>
	screen(route);
