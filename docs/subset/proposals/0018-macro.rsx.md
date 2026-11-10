---
construct: macro.rsx, attr.component, macro.asset, item.use.glob, attr.allow, expr.closure, stmt.let.mut
status: proposed
task: bootstrap round (docs/subset/bootstrap-report.txt)
---

# The Dioxus UI profile

## Rule
In `crates/app` only, these are allowed: `rsx!`, `#[component]` on a component function, `asset!`, `use dioxus::prelude::*`, and `#![allow(clippy::useless_format)]` at the crate root (rsx expands text nodes to `format!`). Closures are allowed as arguments of Dioxus hooks (`use_signal`, `use_future`, `use_hook`). `let mut` is allowed for a `Signal`, whose `set` takes `&mut self`. A component's props follow the core: `live: bool` becomes a two-variant enum. Logic lives in `client` (`FleetState`, `HostLink`), which the UI only renders (`docs/architecture.md` §9).

## Before
No subset-only form exists. Dioxus components are functions that return `rsx!`, with the `#[component]` attribute, and its hooks take closures.

## After
```rust
// crates/app/src/main.rs
#[component]
fn App() -> Element {
    let initial = use_hook(ticket::initial);
    let mut paired = use_signal(|| initial.as_ref().ok().cloned().flatten());
    rsx! {
        document::Stylesheet { href: CSS }
        match paired() {
            Some(ticket) => rsx! { Host { ticket } },
            None => rsx! { Pair { error: first_error, on_pair: move |ticket: Ticket| paired.set(Some(ticket)) } },
        }
    }
}
```

## Measurements
| | tokens | lines |
|---|---|---|
| before | n/a | n/a |
| after | n/a | n/a |
| change | n/a | n/a |

Not measured: there is no version without the construct. The proposal rests on necessity.

Friction points removed: 9 `macro.rsx`, 6 `attr.component`, 1 `macro.asset`, 1 `item.use.glob` (the prelude), 1 `attr.allow`, 7 `expr.closure` sites (hook arguments, and one local `send` closure that becomes a function), and the `let mut` signals in `crates/app/src/main.rs`.

Because macro bodies aren't checked, the reviewer suggests the owner pair this with a review rule: `rsx!` may contain `match`, but no `if`.

## Qualities
1. Named states: kept outside the macro. `rsx!` bodies are not checked (`tools/subset/README.md`, Known limits).
2. Visible branching: weakened inside `rsx!`. The checker can't see branches in markup. The rule keeps decisions in `client`.
3. Forward data flow: weakened, and confined. Signals are reactive state that Dioxus owns. The rule allows them only in the app crate.
4. Errors are values: kept.
5. Local reasoning: kept. A component reads top to bottom as markup.
6. One way: kept. One UI framework idiom, in one crate.

## Owner decision
