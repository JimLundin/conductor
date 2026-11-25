import type {
  Connection as TransportConnection,
  Stream as TransportStream,
  connect,
} from "#transport";

type Stream = Pick<TransportStream, keyof TransportStream>;
type Connection = Pick<TransportConnection, keyof TransportConnection>;

interface Pipe {
  readonly connect: typeof connect;
}

export type { Connection, Pipe, Stream };
