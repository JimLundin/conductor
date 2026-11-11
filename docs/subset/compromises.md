# Where the subset gives ground

The thesis is that **every state is a named enum variant and every decision is
a `match`**, protected by six qualities (`docs/subset.md`). Version 2 accepts
21 proposals. Most keep all six qualities. The ones below don't, or they bend
the thesis at its edges. Each is confined by its rule, and this page is the
list to revisit first when one of them starts to spread.

## Qualities weakened

| Proposal | What gives way | Why it was accepted | What confines it |
|---|---|---|---|
| **0008** `mut` and `&mut self` | **Forward data flow.** An actor's methods change `self` instead of returning a new value. | Without it, the actor rebuilds its whole state on every event. That was 29% more code, and O(n²) as the event log grows. | Only an actor's loop state, and external APIs that take `&mut`. No `+=` anywhere. Every other function builds values. |
| **0018** Dioxus (`rsx!`, signals) | **Visible branching** and **forward data flow**, inside the UI crate. Markup can branch where the checker can't see, and signals are reactive state that Dioxus owns. | There is no other way to build the UI. | `crates/app` only. Decisions are made in `client`, and markup only renders their results. |
| **0009** trait impls | **Local reasoning**, slightly. A library calls our code from somewhere else. | There's no way around it for `Drop`, hand-written `Debug`/serde, and framework hooks such as iroh's `ProtocolHandler`. | A closed list of traits. `Display`, `From` and `FromStr` are written as plain methods instead. |
| **0011** `#[from]` | **Local reasoning**, slightly: the error conversion isn't written at the `?`. **One way** is at risk, since `?` alone and `map_err` both exist. | It is 17% shorter across 20 sites. | Only when exactly one variant holds that foreign error type. Otherwise use `map_err`, so a fact about the enum decides between the two, not taste. |

## The thesis bends at its edges

- **`bool` comes back at the boundary.** 0021 allows `true`/`false` as arguments to external APIs, and 0019 allows a `bool` for a command-line flag. Neither reaches our own types, but the rule "no `bool`" is now "no `bool` past the edge".
- **Branching outside `match`.** 0016 `cfg` is a compile-time branch. It is kept visible by being item-level and paired. 0006 `.await` and 0007 `loop` add suspension and repetition outside iterator adapters, both needed for actors and I/O.
- **Iteration outside adapters.** The actor `loop` (0007) is the one loop. Every other loop is still an adapter.

## Enforcement is partly by review

`tools/subset` checks the location rules as `[[scope]]` tables in
`subset.toml`, the shape rules it can see in the syntax, and the interior
mutability types (`tools/subset/README.md`). It has no type information and
doesn't expand macros, so a reviewer still checks these:

- **Whose type it is.** The tool knows our types and functions only by name.
  0008(b) `let mut` for an external value is approximated as "never
  reassigned", 0011 "foreign type" and 0021 "external API" as "not a name we
  define", and a method call's receiver is ours only when it is `self`.
  0021's "the parameter is declared `bool`, not a generic we fill with
  `bool`" needs types.
- **Why a construct is there.** 0002 `pub` only for what another crate
  uses, 0006 `async fn` only for I/O, 0012 serde only on types that cross a
  wire (the tool checks the location), 0013 generics only at the wire
  boundary (the tool checks the bound), 0014 a slice only for a read-only
  sequence, 0015 a byte array only where its size is part of a format, and
  0009 generics only where the trait's signature demands them (the tool
  allows any inside a listed trait impl).
- **Macro bodies.** 0007's "each `select!` arm is a single call", Dioxus
  markup (0018) and `tracing` fields are not parsed.
- **What a body does.** 0017's callback only forwards the event to a
  channel, 0018's logic lives in `client`, and 0019's `main` turns the flag
  into an enum before passing it on.
- **Pairs by name.** 0016 counts a `cfg` fn or impl as paired when the other
  platform's item has the same name, and any `use` pairs with any `use`. A
  `cfg` module needs no pair, since the rule exempts modules that exist on
  one platform only.
