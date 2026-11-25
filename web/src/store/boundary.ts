import type { ClientName } from "@conductor/core/schemas/index";
import type { KeyValueStore } from "effect/persistence";
import type { Pipe } from "#ui/pipe.ts";

interface Boundary {
  readonly client: ClientName;
  readonly fragment: () => string;
  readonly go: (fragment: string) => void;
  readonly pipe: Pipe;
  readonly replace: (fragment: string) => void;
  readonly storage: KeyValueStore.KeyValueStore;
}

export type { Boundary };
