# The client link is JSON, typed from `proto`

Frames between the browser and a host are JSON, serialized by serde, and the app decodes them with Effect Schemas generated from the Rust `proto` types, so the wire has one definition and a change to `proto` breaks the app's build instead of its runtime. postcard stays where only Rust reads it: host stores and traffic between hosts. Signed fleet payloads are signed over their exact JSON bytes and carried verbatim; every verifier checks the bytes before parsing them, since JSON has no canonical encoding.

## Rejected

- **A postcard codec in TypeScript.** postcard is not self-describing, so a hand-written decoder would be a second definition of every wire type that drifts without failing.
