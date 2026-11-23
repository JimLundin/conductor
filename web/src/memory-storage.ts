// An in-memory Web Storage for tests, holding `entries` to start with.
export const memoryStorage = (
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
