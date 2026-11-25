interface ValueChange {
  readonly target: Readonly<Pick<HTMLInputElement, "value">>;
}

export type { ValueChange };
