import { Data } from "effect";

// The screen a URL fragment names. Pages serves one path, so every screen is a fragment.
export type Route = Data.TaggedEnum<{
	readonly Pairing: Record<never, never>;
}>;

export const Route = Data.taggedEnum<Route>();

// With no host paired, every fragment shows pairing.
export const route = (_fragment: string): Route => Route.Pairing();
