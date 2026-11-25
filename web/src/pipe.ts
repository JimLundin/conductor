// The browser's transport to a host: iroh compiled to wasm, as crates/web-iroh declares it.
import type * as Transport from "#transport";

export type Stream = Pick<Transport.Stream, keyof Transport.Stream>;
export type Connection = Pick<Transport.Connection, keyof Transport.Connection>;

export interface Pipe {
  readonly connect: typeof Transport.connect;
}
