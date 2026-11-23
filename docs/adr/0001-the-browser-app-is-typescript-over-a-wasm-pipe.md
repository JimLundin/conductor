# The browser app is TypeScript over a wasm iroh pipe

The browser app is TypeScript with React and Effect in `web/`, replacing the Dioxus app, so the back end is not bound to front-end crates and the front end iterates on its own. Its only Rust is `crates/web-iroh`, iroh compiled to wasm and exposed as connections and byte streams with no knowledge of `proto`, because the browser has no other way to reach a host end to end over the relay. The link protocol and the fold of a host's events into what the app shows live only in TypeScript; the daemon's tests assert on the event stream instead of keeping a Rust client.

## Rejected

- **No Rust in the browser now.** iroh's end-to-end encryption is QUIC and TLS bound to the host's key; a TypeScript transport needs a new way for a host to be reached, which is open as its own research decision. The pipe is shaped so one could replace it without touching the app.
- **A wasm module exporting the link and the fold.** It would keep front-end decisions in Rust, and a second client in TypeScript would duplicate them.
