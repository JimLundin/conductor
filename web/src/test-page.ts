// A page for the store in tests: in-memory storage, a fragment the test reads, and a pipe.
import { Effect } from "effect";
import { dialing } from "./link/replay.ts";
import type { Pipe } from "./pipe.ts";
import type { Boundary, Store } from "./store.ts";

// An in-memory Web Storage, holding `entries` to start with.
const memoryStorage = (
	entries: readonly (readonly [string, string])[] = [],
): Storage => {
	const items = new Map(entries);
	return {
		get length(): number {
			return items.size;
		},
		key: (index: number): string | null => [...items.keys()][index] ?? null,
		getItem: (key: string): string | null => items.get(key) ?? null,
		setItem: (key: string, value: string): void => {
			items.set(key, value);
		},
		removeItem: (key: string): void => {
			items.delete(key);
		},
		clear: (): void => {
			items.clear();
		},
	};
};

const keysOf = (storage: Storage): readonly string[] =>
	Array.from(
		{ length: storage.length },
		(_, index) => storage.key(index) ?? "",
	);

interface Page {
	readonly boundary: Boundary;
	// The fragment the page shows now.
	readonly at: () => string;
}

// A page at `start`, whose links dial `pipe`; by default one that never connects.
const page = (
	start: string,
	storage: Storage = memoryStorage(),
	pipe: Pipe = dialing([]),
): Page => {
	let fragment = start;
	const move = (next: string): void => {
		fragment = next;
	};
	return {
		boundary: {
			pipe,
			client: "test-client",
			storage,
			fragment: (): string => fragment,
			go: move,
			replace: move,
		},
		at: (): string => fragment,
	};
};

// Resolves once `holds` is true of the store.
const when = (
	store: Store,
	holds: (store: Store) => boolean,
): Effect.Effect<void> =>
	Effect.callback<void>((resume) => {
		if (holds(store)) {
			resume(Effect.void);
			return;
		}
		const stop = store.subscribe(() => {
			if (holds(store)) {
				stop();
				resume(Effect.void);
			}
		});
	});

export { keysOf, memoryStorage, page, when };
