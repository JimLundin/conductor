# Domains own their types, and the client link composes them

Each domain crate owns its model, its commands, its results and its refusals, and knows nothing of the wire; the client link's crate, `link`, depends on the domains and wraps their enums in its envelope (`Command::Issue(IssueCommand)`). This replaces `proto`, one crate under every other that held all the domains' types beside the envelope and joined them in flat cross-domain enums, so a change to any domain touched code in all of them. The single definition that this project's ADR 0002 and the home's ADR 0003 require still holds: every type is defined once, in the crate that gives it meaning, and the app's schemas are generated from the link's root types wherever they live.

## Rejected

- **A shared crate for values with no domain**, such as a host's path or a command's id. Every type has a domain that gives it meaning; when two domains model something similar, each owns its own type, and the crate that depends on both converts between them.

## Consequences

- The client link's frames nest one level per domain, and its version string still covers every type reachable from its roots, so a change to a domain's commands still bumps it.
- The flow's types live in `home` again; they were moved to `proto` only so the Rust app could receive them.
