// The browser's transport to a host: iroh compiled to wasm, as crates/web-iroh declares it.
import type { connect } from "../web-iroh/web_iroh.js";

export type { Connection, Stream } from "../web-iroh/web_iroh.js";

export interface Pipe {
	readonly connect: typeof connect;
}
