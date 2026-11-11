# The Conductor Rust subset

Conductor code is written in a small subset of Rust that grows on evidence.
The subset starts from one idea: **every state is a named enum variant, and
every decision is a `match` over it.** There are no boolean checks outside
a predicate closure (proposal 0028). Anything not listed in `subset.toml` is
not allowed. A construct joins the list only when a reviewer shows, with
measurements, that it shrinks real code without costing any of the
qualities below, and the owner accepts it.

`subset.toml` is the source of truth. `tools/subset check` enforces it on
the paths under `include`. `tools/subset score` and `tools/subset measure`
give the weighted score and the token count that proposals report (see
Weights). `tools/subset report` summarises violations by construct. A `[[scope]]` in `subset.toml` allows some ids only under the
paths it lists, such as Dioxus only in `crates/app`.

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
7. **Repetition is data.** When two or more places share a shape and
   differ only in values, the values become data and the shape is written
   once. The forms are:
   - a `const` table of named structs with one adapter chain over it, as
     in `const PROVIDERS: &[Provider] = &[..]` and
     `PROVIDERS.iter().find(|p| p.name == name)`;
   - one `match` that maps a variant to its data, used by every call site,
     as in `fn label(kind: Kind) -> &'static str { match kind { .. } }`;
   - enum variants that carry their data, as in `Exit::Code(i32)` rather
     than one variant per code;
   - test cases as a table with one assertion over them.

## Core

The ids are those used in `subset.toml`.

| Area | Allowed | Notes |
|---|---|---|
| Data | `item.struct.named`, `item.struct.newtype`, `item.enum` | `bool` is not a type in our code: a two-state value is a two-variant enum. IDs and units are newtypes. |
| Behaviour | `item.fn`, `item.impl.inherent`, `item.const` | Free functions and inherent methods. |
| Branching | `expr.match`, `pat.*` | Matches are exhaustive. `true` / `false` are not patterns, because that would be a boolean check written as a match. A single-branch `if let` (0027) is the one other branching form. |
| Building values | `expr.call`, `expr.method_call`, `expr.field`, `expr.struct`, `expr.path`, `expr.lit`, `expr.reference`, `expr.block`, `expr.paren` | |
| Errors | `expr.try`, plus `Result` / `Option` through the core | `?` is the only early exit. |
| Iteration | `expr.closure.adapter` | Collections are processed only with iterator adapters (`map`, `filter_map`, `try_fold`, `collect`, and so on). The closure must be an argument of an iterator, `Option` or `Result` method. |
| Predicates | `macro.matches`, `expr.binary.{eq,ne,lt,le,gt,ge,and,or}`, `expr.unary.not` | Proposal 0028. A closure passed straight to `filter`, `find`, `any`, `all`, `position`, `take_while`, `skip_while` or `retain` may have a boolean expression as its body: `matches!`, a comparison, `&&`, `\|\|`, `!`. Nowhere else, and never a `true`/`false` literal. Counting is `filter(\|x\| matches!(x, ..)).count()`. |
| Bindings | `stmt.let` | Immutable, with a pattern. |
| Tuples | `expr.tuple`, `pat.tuple`, `type.tuple` | Proposal 0001. Matching on several values at once (`match (state, event)`) and short-lived pairs. A value that outlives its expression or crosses a public signature gets a named struct, so states stay named. |
| Crate API | `vis.pub` | Proposal 0002. `pub` marks only what another crate uses, exported from the crate root. |
| Crate-internal sharing | `vis.restricted` | Proposal 0003. Only `pub(crate)`, never `pub(super)` or `pub(in …)`, and a `pub(crate)` struct has no `pub` fields. |
| Changed copies | `expr.struct.update` | Proposal 0004. `..base` only where the base is an existing value of the same type or an external crate's `Default::default()` or named constructor. |
| Unit | `type.unit`, `expr.unit` | Proposal 0005, with its boundary part replaced by 0023. `()` only at a library boundary (0023), which covers `Result<(), E>` and `Ok(())`, and as `fn main`'s return type. |
| Async | `item.fn.async`, `expr.await`, `expr.async`, `attr.tokio::main` | Proposal 0006. `async fn` for I/O, and an `async` block only as a spawn argument or the whole body of a stream closure. |
| Actor loop | `expr.loop`, `expr.break`, `macro.allowed` for `tokio::select` | Proposal 0007. `loop` only as the whole body of an actor's `run` fn or a drive fn, taking one input per turn and handling it with one call or `match`. |
| Loop state | `item.fn.receiver.mut`, `stmt.let.mut`, `pat.ident.mut`, `expr.assign`, `expr.reference.mut`, `type.reference.mut` | Proposal 0008, widened in version 4. Mutation only of an actor's loop state, of a value that loop state owns (its methods may take `&mut self`, called on `self.field`), and of external values whose API takes `&mut`. Never a `&mut` parameter of our own types, and never compound assignment. |
| Library traits | `item.impl.trait`, `generics.lifetime`, `type.lifetime` | Proposal 0009. Trait impls only for `Drop`, `Debug`, `Serialize`, `Deserialize` and `ProtocolHandler`, with lifetimes only where the trait's signature demands them. |
| Error enums | `attr.derive` of `thiserror::Error`, `attr.error` | Proposal 0010. `thiserror::Error` on an error type, with one `#[error("…")]` message per variant. |
| Error conversion | `attr.from` | Proposal 0011. `#[from]` only on the one variant of an enum that holds a given error type, ours or a library's. |
| Wire types | `attr.derive.Serialize`, `attr.derive.Deserialize`, `attr.serde` | Proposal 0012. serde derives only in `crates/proto` and the agents' provider protocol modules, with `transparent`, `tag`, `content`, `rename_all` and `rename` only. |
| Codec generics | `generics.type_param` | Proposal 0013. A type parameter only on a codec fn, with one `Serialize` or `DeserializeOwned` bound and no `where`. |
| Slices | `type.slice` | Proposal 0014, amended in version 4. `&[T]` for a read-only sequence parameter, and a constant list is always `const NAME: &[T] = &[..]`. |
| Arrays | `type.array`, `expr.array`, `expr.repeat` | Proposal 0015, amended in version 4. `[T; N]` only for a fixed size that is part of a format or API, such as `[u8; 32]` keys, and a literal list passed straight to a call (`f([a, b])` or `f(&[a, b])`). `const` tables became `const` lists under 0014. |
| Platform code | `attr.cfg` | Proposal 0016. `cfg` only on `target_family = "wasm"` and `unix` (or their `not`), on items, each with its counterpart. |
| Browser callbacks | `type.dyn`, `expr.closure` | Proposal 0017. `Closure<dyn FnMut(..)>` only in `crates/client/src/platform/`, and its closure only forwards the event to a channel. |
| UI | `macro.rsx`, `macro.asset`, `attr.component`, `item.use.glob`, `attr.allow` | Proposal 0018. Dioxus only in `crates/app`, which renders what `client` decides. |
| Command line | `attr.derive.Parser`, `attr.derive.Subcommand`, `attr.derive.ValueEnum`, `attr.command`, `attr.arg`, `type.bool` | Proposal 0019. clap only in a binary's `main.rs` and in `examples/`, and a flag `bool` becomes an enum before it leaves `main`. |
| Async tests | `attr.tokio::test` | Proposal 0020. `#[tokio::test]` with no arguments on an `async fn` test. |
| External flags | `expr.lit.bool` | Proposal 0021, replaced by 0023. `true` / `false` only at a library boundary (0023). |
| Library boundary | `type.unit`, `expr.unit`, `type.bool`, `expr.lit.bool`, `expr.reference.mut`, `type.reference.mut` | Proposal 0023. Where a library API demands `()`, `bool` or `&mut`, the shape is allowed in a type argument, call argument or field value of something we don't define, and only there. It never becomes a field, a variant payload, a parameter or a return type of our own items. |
| Ordered newtypes | `attr.derive` of `PartialOrd`, `Ord` | Proposal 0024. Only on a newtype whose order means something, such as `HostSeq`. Callers compare with `a.cmp(&b)` and match the `Ordering`, never through `.0`. |
| Request helper | `type.impl_trait.arg`, `generics.type_param`, `expr.closure` | Proposal 0026. Each actor handle may have one private `ask<T>` helper taking `build: impl FnOnce(oneshot::Sender<T>) -> Msg`, and every request/reply round trip goes through it. Nothing else takes an `impl Trait` argument. |
| One-case branch | `expr.if_let.single` | Proposal 0027. `if let Pattern = expr { … }` with no `else`, no `else if` and no `&&` let-chain, the one form for "act only on this case". Anything with an alternative branch is a `match`. |
| Discarding | `stmt.let` with `_` | Proposal 0025, revised. `let _ = value;` is the one way to discard a value. `drop(..)` only releases a resource early, and `.ok()` is never a discard. No lint is switched off. |
| Build-time constants | `macro.allowed` for `env` | Proposal 0022. `env!` only for `CARGO_PKG_VERSION`, `CARGO_PKG_NAME` and `CARGO_MANIFEST_DIR`. |
| Modules | `item.use`, `item.mod` | |
| Attributes | `attr.doc`, `attr.derive` (the listed derives), `attr.cfg_test` | |
| Macros | `macro.allowed` (the listed macros) | Tests may also use `assert_eq!` and `assert_ne!`. |

**Standard-library booleans.** Some std APIs return `bool`. Prefer the
`Option`- or `Result`-returning form (`first()` over `is_empty()`, `cmp`
over `<`). Where only a `bool` exists, that is a friction point: record it.
Don't work around it.

## Not in the core

Everything else, for example `if`, `if let` with an `else` or a let-chain,
`while`, `for`, `return`, `continue`, `break` outside a drive loop,
operators (`+`, `as`, indexing, ranges, and the boolean operators outside a
predicate closure), compound assignment (`+=`), `let … else`, trait
definitions, generic types, and unlisted macros. Some of these may join
through proposals. Others, such as `unsafe` and interior mutability,
conflict with the qualities and won't.

Where the subset gives ground on the qualities is listed in
[`docs/subset/compromises.md`](subset/compromises.md).

## Weights

`tools/subset score` measures code with a **weighted score**: each token
costs the weight of the innermost construct that contains it, and comments
cost nothing. A construct's weight comes from its **tier**, so code that
leans on what the subset encourages scores lower than code of the same
length that leans on its compromises. The tiers and the id-to-tier map
live in `[weights]` in `subset.toml`, where the owner tunes them.

| Tier | Weight | For |
|---|---|---|
| encouraged | 0.5 | Type definitions (structs, newtypes, enums, the types in their fields), `match` with its arms and patterns, iterator adapter calls, `?`. |
| neutral | 1.0 | The rest of the core: calls, paths, literals, `let`, field access, `pub`, references, slices. An allowed id missing from the map is neutral. |
| tolerated | 2.0 | Accepted compromises: `async` and `.await`, the single `if let` (0027), `let _ =` (0025), predicate closures' boolean expressions (0028), `ask` (0026), generics, lifetimes, trait impls, `cfg` pairs, tuples, the 0023 shapes, primitives in our signatures and fields. |
| costly | 3.0 | Mutation and its kin: `mut`, `&mut self`, assignment (0008), the actor `loop` (0007), `dyn` (0017), `rsx!` (0018), untyped data. |
| outside | 5.0 | Any violation of the current subset, whatever its id, so code not yet rewritten scores badly. |
| boundary | 1.0 | Any construct whose shape a library fixes (below). |

The score of the affected code is what the ratchet compares (Growing the
subset), and proposals show it beside the token count.

**The boundary context.** One standard applies everywhere: a construct
whose shape an item we don't define fixes costs the boundary weight instead
of its tier. "We don't define" uses the same name facts as the 0023 checks,
with a method call ours only when its receiver is `self`. In practice that
covers, for example, a `bool` or `()` passed to a library function, a
`let mut` that exists to lend a value to a library method as `&mut`, an
impl of a library trait with the signature parts it fixes, library derives
and their helper attributes, `Closure<dyn FnMut(..)>`, and Dioxus markup
and signals. These are examples, not a closed list. Our own `&mut self`, an
actor's loop state and assignment keep their tier. `subset score` shows the
boundary share at every level, so library glue isn't read as style debt.

### Typing

The score also rewards naming data with types.

- **Defining a type is cheap.** A struct with its fields, a newtype and an
  enum with its variants are encouraged, with the types in their fields.
- **Primitives in our own signatures and fields are tolerated.** `String`,
  `&str`, the integer and float types and `Vec<u8>`, as a field type of our
  own struct or variant or as a parameter or return type of our own fn.
  Inside fn bodies they are neutral. A newtype's one field is exempt, since
  it is where the primitive belongs.
- **Untyped data is costly**, wherever it appears: `serde_json::Value`,
  `HashMap<String, _>` or `BTreeMap<String, _>`, and `dyn Any`.
- **Wire types of an external protocol are boundary.** A field of a type
  with a library derive in a module listed in `external_protocols` mirrors
  someone else's format, such as the provider CLI messages in
  `crates/agents`. `crates/proto` is our own format, so the normal rules
  apply there.

`tools/subset/README.md` gives the full model and the configuration.

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

The owner accepted 0002 to 0022 as version 2, then 0023 and the 0011
amendment as version 3. Version 4 accepts 0024, 0026, 0027 and 0028,
accepts 0025 in a revised form, widens 0008 and amends 0014 and 0015.
The rewrite works from `docs/subset/rewrite-report.txt`, the report under version 4.

## Growing the subset

The pair loop (`.claude/skills/pair/SKILL.md`) is the only way in:

1. An **implementer** writes the task in the current subset and logs each
   **friction point** in the code: a place where a missing construct made
   the code longer or more roundabout.
2. A **reviewer** checks compliance and the qualities. For each candidate
   construct, it rewrites the affected code with that construct, measures
   both versions with `tools/subset score` and `tools/subset measure`, and
   judges the qualities.
3. A candidate that lowers the weighted score and keeps every quality
   becomes a **proposal** in `docs/subset/proposals/NNNN-<id>.md`: the
   construct, the before and after code, the measurements, the quality
   assessment, the rule for when to use it, and its **weight**: the tier
   the construct takes and whether its boundary use applies.
4. The **owner** accepts or rejects it. An accepted id is added to
   `subset.toml` with a line in the table above and its tier in
   `[weights.ids]`, its rule becomes part of the subset, and `version` goes
   up by one.

A review may also propose **changing an existing weight**, as a proposal
of its own with evidence from the reviewed code. The owner decides it the
same way.

Rejected proposals stay in `docs/subset/proposals/`, so a construct is
not proposed twice for the same reason.
