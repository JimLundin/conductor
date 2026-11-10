# The Conductor Rust subset

Conductor code is written in a small subset of Rust that grows on evidence.
The subset starts from one idea: **every state is a named enum variant, and
every decision is a `match` over it.** There are no boolean checks. Anything
not listed in `subset.toml` is not allowed. A construct joins the list only
when a reviewer shows, with measurements, that it shrinks real code without
costing any of the qualities below, and the owner accepts it.

`subset.toml` is the source of truth. `tools/subset check` enforces it on
the paths under `include`. `tools/subset measure` counts the code volume
used in proposals. `tools/subset report` summarises violations by
construct.

## Qualities

These are what the subset protects. A proposal that weakens any of them
is rejected, however much code it saves.

1. **Named states.** Every state a value can be in is a type or an enum
   variant. A reader never decodes a `bool`, a sentinel value or a
   combination of flags.
2. **Visible branching.** Every decision is a `match` with all of its arms
   written out. Control flow does not hide in operators, early exits or
   short-circuiting.
3. **Forward data flow.** Values are built, never changed: no mutable
   bindings and no interior mutability. A function's output depends only on
   its inputs.
4. **Errors are values.** Failure is a `Result` or an `Option`, passed on
   with `?` or matched like any other enum.
5. **Local reasoning.** A function can be understood from its own text and
   the types it names.
6. **One way.** Each kind of task has one idiom in the subset, so equal
   problems read alike.

## Core

The ids are those used in `subset.toml`.

| Area | Allowed | Notes |
|---|---|---|
| Data | `item.struct.named`, `item.struct.newtype`, `item.enum` | `bool` is not a type in our code: a two-state value is a two-variant enum. IDs and units are newtypes. |
| Behaviour | `item.fn`, `item.impl.inherent`, `item.const` | Free functions and inherent methods. |
| Branching | `expr.match`, `pat.*` | Matches are exhaustive. `true` / `false` are not patterns, because that would be a boolean check written as a match. |
| Building values | `expr.call`, `expr.method_call`, `expr.field`, `expr.struct`, `expr.path`, `expr.lit`, `expr.reference`, `expr.block`, `expr.paren` | |
| Errors | `expr.try`, plus `Result` / `Option` through the core | `?` is the only early exit. |
| Iteration | `expr.closure.adapter` | Collections are processed only with iterator adapters (`map`, `filter_map`, `try_fold`, `collect`, and so on). The closure must be an argument of an iterator, `Option` or `Result` method. |
| Bindings | `stmt.let` | Immutable, with a pattern. |
| Tuples | `expr.tuple`, `pat.tuple`, `type.tuple` | Proposal 0001. Matching on several values at once (`match (state, event)`) and short-lived pairs. A value that outlives its expression or crosses a public signature gets a named struct, so states stay named. |
| Modules | `item.use`, `item.mod` | |
| Attributes | `attr.doc`, `attr.derive` (the listed derives), `attr.cfg_test` | |
| Macros | `macro.allowed` (the listed macros) | Tests may also use `assert_eq!` and `assert_ne!`. |

**Standard-library booleans.** Some std APIs return `bool`. Prefer the
`Option`- or `Result`-returning form (`first()` over `is_empty()`, `cmp`
over `<`). Where only a `bool` exists, that is a friction point: record it.
Don't work around it.

## Not in the core

Everything else, for example `()`, `if`, `while`, `for`, `loop`, `return`,
`break`, operators (`&&`, `==`, `+`, `as`, indexing, ranges), `let mut`,
`let … else`, traits, generics, `async`, `dyn`, and unlisted macros. Some of
these will join through proposals. Others, such as `unsafe` and
interior mutability, conflict with the qualities and won't.

## Bootstrap round

The code from before the subset uses constructs the core lacks, and some
of it (such as `async` and trait impls) has no long-hand form. The first
pass of the pair loop therefore runs on the existing code:

1. `tools/subset report` lists every construct outside the subset under
   `include`, with counts and locations.
2. The reviewer turns that report into proposals. Each proposal gives the
   minimal construct and the narrowest rule that the existing code needs,
   with before/after examples taken from that code. It also proposes
   rewrites into the core for everything the core can already express.
3. The owner decides. The implementer then rewrites module by module in
   the agreed subset, until `tools/subset check` passes on all of
   `crates/`.

## Growing the subset

The pair loop (`.claude/skills/pair/SKILL.md`) is the only way in:

1. An **implementer** writes the task in the current subset and logs each
   **friction point** in the code: a place where a missing construct made
   the code longer or more roundabout.
2. A **reviewer** checks compliance and the qualities. For each candidate
   construct, it rewrites the affected code with that construct, measures
   both versions with `tools/subset measure`, and judges the qualities.
3. A candidate that saves volume and keeps every quality becomes a
   **proposal** in `docs/subset/proposals/NNNN-<id>.md`: the construct,
   the before and after code, the measurements, the quality assessment,
   and the rule for when to use it.
4. The **owner** accepts or rejects it. An accepted id is added to
   `subset.toml` with a line in the table above, its rule becomes part of
   the subset, and `version` goes up by one.

Rejected proposals stay in `docs/subset/proposals/`, so a construct is
not proposed twice for the same reason.
