# Where the subset gives ground

The thesis is that **every state is a named enum variant and every decision is
a `match`**, protected by seven qualities (`docs/subset.md`). Version 4 accepts
27 proposals. Most keep all the qualities. The ones below don't, or they bend
the thesis at its edges. Each is confined by its rule, and this page is the
list to revisit first when one of them starts to spread.

## Qualities weakened

| Proposal | What gives way | Why it was accepted | What confines it |
|---|---|---|---|
| **0008** `mut` and `&mut self` | **Forward data flow.** An actor's methods change `self` instead of returning a new value. | Without it, the actor rebuilds its whole state on every event. That was 29% more code, and O(n²) as the event log grows. | Only an actor's loop state, a value that loop state owns (version 4: a record or buffer held in one of its fields, changed through that value's own `&mut self` methods), and external APIs that take `&mut`. No `&mut` parameter of our own types, and no `+=` anywhere. Every other function builds values. |
| **0018** Dioxus (`rsx!`, signals) | **Visible branching** and **forward data flow**, inside the UI crate. Markup can branch where the checker can't see, and signals are reactive state that Dioxus owns. | There is no other way to build the UI. | `crates/app` only. Decisions are made in `client`, and markup only renders their results. |
| **0009** trait impls | **Local reasoning**, slightly. A library calls our code from somewhere else. | There's no way around it for `Drop`, hand-written `Debug`/serde, and framework hooks such as iroh's `ProtocolHandler`. | A closed list of traits. `Display`, `From` and `FromStr` are written as plain methods instead. |
| **0011** `#[from]` | **Local reasoning**, slightly: the error conversion isn't written at the `?`. **One way** is at risk, since `?` alone and `map_err` both exist. | It is 17% shorter across 20 sites. | Only when exactly one variant holds that error type, ours or a library's. Otherwise use `map_err`, so a fact about the enum decides between the two, not taste. |

## The thesis bends at its edges

- **Library shapes come back at the boundary.** 0023 allows `()`, `bool`, `true`/`false` and library `&mut` where an API we don't define demands them: in a type argument, call argument or field value of something we don't define, as in `EventHandler<()>`, `watch::channel(false)` and `tx.send(())`. 0019 also allows a `bool` for a command-line flag. Neither reaches a field, variant payload, parameter or return type of our own items, but the rule "no `bool`" is now "no `bool` past the edge".
- **Branching outside `match`.** 0016 `cfg` is a compile-time branch. It is kept visible by being item-level and paired. 0006 `.await` and 0007 `loop` add suspension and repetition outside iterator adapters, both needed for actors and I/O.
- **A second branching form.** 0027 allows `if let` for "act only on this case", because clippy's `single_match` rejects the `match` with an empty arm and no lint is switched off (0025 revised). It is confined to one branch: with an `else`, an `else if` or a let-chain it is a `match` again, so every two-way decision is still a `match`.
- **`bool` comes back inside predicates.** 0028 lets a closure passed straight to `filter`, `find`, `any`, `all`, `position`, `take_while`, `skip_while` or `retain` return a boolean expression: `matches!`, a comparison, `&&`, `||` or `!`. This is the one place a decision is written as a `bool` rather than a `match`. It is confined to the closure body, the adapter is the branch, and the `bool` never becomes a binding, field, parameter or return type of ours. `true` and `false` stay out of it.
- **Iteration outside adapters.** The actor `loop` (0007) is the one loop. Every other loop is still an adapter.

## Enforcement is partly by review

`tools/subset` checks the location rules as `[[scope]]` tables in
`subset.toml`, the shape rules it can see in the syntax, and the interior
mutability types (`tools/subset/README.md`). It has no type information and
doesn't expand macros, so a reviewer still checks these:

- **Whose type it is.** The tool knows our types and functions only by name.
  0008(b) `let mut` for an external value is approximated as "never
  reassigned", and 0023's "something we don't define" as "not a name we
  define", with a method call's receiver ours only when it is `self`. A
  library wrapper hides where a value ends up: the tool allows
  `Option<bool>` as a field of our struct and `Some(true)` as its value,
  because `Option` and `Some` aren't ours. The reviewer checks that a
  boundary value doesn't flow on into our types. 0008's "a value that loop
  state owns" is approximated as "a type of ours named in a field type of a
  loop-state type", one step down, with enums of ours transparent so
  their variant payloads count too. 0024's "never compare through `.0`" is
  checked only where the receiver is `self` in an impl of an ordered
  newtype or a parameter declared as one. 0028's predicate leaves aren't
  typed, so `|x| x.flag` on a library `bool` passes.
- **Why a construct is there.** 0002 `pub` only for what another crate
  uses, 0006 `async fn` only for I/O, 0012 serde only on types that cross a
  wire (the tool checks the location), 0013 generics only at the wire
  boundary (the tool checks the bound), 0014 a slice only for a read-only
  sequence, 0015 a byte array only where its size is part of a format,
  0009 generics only where the trait's signature demands them (the tool
  allows any inside a listed trait impl). Also 0024 an order that means
  something (the tool checks that the type is a newtype), 0025 `drop` only
  of a value that owns a resource released early (the tool checks that the
  argument is a plain name, and flags `drop` passed as a function), 0026 every round trip going through `ask` and
  its closure only building the variant (the tool checks the helper's
  shape), and 0028 a `fold` or `map(..).sum()` used only to count (the tool
  flags `filter_map(..).count()`).
- **Macro bodies.** 0007's "each `select!` arm is a single call", Dioxus
  markup (0018) and `tracing` fields are not parsed.
- **What a body does.** 0017's callback only forwards the event to a
  channel, 0018's logic lives in `client`, and 0019's `main` turns the flag
  into an enum before passing it on.
- **Pairs by name.** 0016 counts a `cfg` fn or impl as paired when the other
  platform's item has the same name, and any `use` pairs with any `use`. A
  `cfg` module needs no pair, since the rule exempts modules that exist on
  one platform only.
