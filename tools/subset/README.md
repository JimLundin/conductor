# subset

`subset` enforces and measures the Conductor Rust subset described in
`docs/subset.md`. The allowlist lives in `subset.toml` at the repository
root.

## Usage

Run it from the repository root with `cargo run -q -p subset -- <command>`.

```sh
subset check [--config subset.toml]
subset report [--config subset.toml] [--json]
subset measure <paths...> [--json]
subset score [paths...] [--config subset.toml] [--json] [--by crate|file|fn|construct]
subset report-html --out <dir> [--config subset.toml] [--compare <dir>]... [--history <n>]
subset ids
subset score-ids
subset rules
```

- `check` parses every file that matches `include` and prints one line per
  construct that isn't allowed, as `path:line:col: <construct-id>: <reason>`.
  Paths in `include` are relative to the config file's directory. It exits
  with 0 when the files are clean, 1 when there are violations, and 2 on a
  config or parse error. The reason names the proposal's rule where one
  applies, as described under Scopes.
- `report` groups the same violations by id. Each line gives the count, the
  number of files and up to five example locations, most frequent id first.
  `--json` prints `{"total", "ids": [{"id", "reason", "count", "files",
  "examples"}]}`. The bootstrap round in `docs/subset.md` starts from this
  report.
- `measure` counts the code volume of files, or of every `*.rs` file under a
  directory, per file and in total. Proposals use it to compare the before
  and after versions of the same code.
- `score` prints the weighted score of `paths`, or of the files under
  `include` when none are given, at one level: per crate (the default), per
  file, per fn or per construct. Each row gives the score, the tokens, the
  lines, the score per token, the boundary score and the boundary's share
  of the score. Lines are counted as `measure` counts them. A construct
  row's lines are those on which the construct owns a token, so construct
  rows overlap and don't add up to the total. Fns and constructs are
  sorted by score, highest first. `--json` prints `{"by", "rows":
  [{"name", "score", "tokens", "lines", "per_token", "boundary",
  "boundary_share"}], "total"}`, with `count` and `tier` on construct
  rows. See "The weighted score" below.
- `report-html` writes `<dir>/index.html`, a static page that breaks the
  score down with charts. See "The HTML report" below.
- `score-ids` prints the table of score-only ids under "The weighted
  score", kept equal to this file the same way as `ids`.
- `ids` prints the id table below. A test keeps the table in this file equal
  to that output, so add an id to `src/ids.rs` and then paste the new table
  here.
- `rules` prints the shape rule table below, kept equal the same way from
  `src/rules.rs`.

`tools/check.sh` runs `subset check` when `SUBSET=1` is set. The flag goes
away once `crates/` passes, and then the check runs on every commit.

## Scopes

A `[[scope]]` table allows ids only in the files its `paths` cover:

```toml
[[scope]]
rule = "Dioxus only in crates/app (proposal 0018)"
paths = ["crates/app/**"]
allowed = ["macro.rsx", "macro.asset", "attr.component"]
```

`paths` are globs relative to the config file's directory, where `*` stays
within one path component and `**` crosses any number. A file may use the
global `allowed` ids and the ids of every scope that covers it. A use of a
scoped id in any other file is reported with the scope's `rule` as its
reason. Ids that a proposal forbids outright, such as compound assignment
and interior mutability, are reported with that proposal's rule too.

## Shape rules

An accepted proposal often allows an id only in some form or position, such
as `loop` only as the whole body of an actor's `run` fn (0007). `check`
reports a use that breaks such a rule with the id and the rule as its
reason:

```text
crates/daemon/src/host.rs:12:5: expr.loop: loop must be the whole body of an actor run fn, after at most one `let mut` state (proposal 0007)
```

A rule is checked only where its id is allowed, by `allowed` or by a scope.
Where the id isn't allowed, the id itself is the violation. Some rules need
facts from other files, so `check` first gathers, from every file under
`include`, the names of our crates, modules, types and free functions, the
types used as a `Result`'s error, the actor loop-state types (the `Self`
of a `run(mut self)` whose body is an actor loop, and the type a loop's
`let mut` state is built from), the types of ours named in a loop-state
type's fields or in the variants of an enum of ours it owns, and the
newtypes that derive `Ord`. The facts
are names, not resolved paths. Within one file, a bare type name is ours
only when the file defines it, imports it from a path of ours, or has a
glob import of a path of ours. A name the file imports from a library is
never ours, even when one of our crates defines a type of the same name.

Some rules read a list of names from the config, so a project tunes them
without changing the tool:

```toml
# Types of ours whose methods may take `&mut self` anywhere (0035).
endpoints = ["SendHalf", "RecvHalf"]
```

- `endpoints` names types of ours, matched on the impl's type name, whose
  methods may take `&mut self` wherever they are, not only as loop state.
  The default is none.
- `predicate_adapters` names the methods whose closure argument may have a
  boolean expression as its body (0028, 0037). The default is `filter`,
  `find`, `any`, `all`, `position`, `take_while`, `skip_while` and `retain`.
- `closure_methods` names the other methods a closure may be passed to
  (`expr.closure.adapter`). A closure passed to a method on neither list is
  flagged. The default is the std `Iterator`, `Option` and `Result` methods
  that take a closure: `and_then`, `filter_map`, `find_map`, `flat_map`,
  `fold`, `for_each`, `get_or_insert_with`, `inspect`, `inspect_err`,
  `is_err_and`, `is_none_or`, `is_ok_and`, `is_some_and`, `map`, `map_err`,
  `map_or`, `map_or_else`, `map_while`, `max_by`, `max_by_key`, `min_by`,
  `min_by_key`, `ok_or_else`, `or_else`, `partition`, `reduce`, `rfind`,
  `rposition`, `scan`, `try_fold`, `try_for_each` and `unwrap_or_else`.

- `fallible_operators` maps each operator trait whose operation can
  overflow or panic to its methods (0038). A `use` of a listed trait, a
  path through it, and a call of one of its methods in a file that imports
  the trait are flagged. The default is `std::ops::Add`, `Sub`, `Mul`,
  `Div`, `Rem`, `Neg`, `Shl` and `Shr`, each with its lowercase method, and
  their `*Assign` forms with `add_assign` and so on.
- `consuming_readers` names types and traits that read by changing their
  buffer (0039). A `use` of one and a path through one are flagged. The
  default is `bytes::Buf`.

Both lists hold paths, matched against paths as the code writes them: the
path's segment names the item, and the segments written before it agree
with the listed path, so `ops::Add` matches `std::ops::Add`. `core::` and
`alloc::` read as `std::`. A bare name that the file defines or imports
from a path of ours is not the listed item. Method calls are matched by
name, which is ambiguous without types, so 0038 matches them only in a
file that imports the trait by name or through a glob of its module: a
method `add` of ours is flagged in such a file, and a trait brought in
through a library prelude's glob is not seen.

A list that is given replaces its default. To extend a default, copy it.

| Id | Rule |
|---|---|
| `vis.restricted` | only `pub(crate)`, never `pub(super)` or `pub(in …)` (proposal 0003) |
| `vis.pub` | a field of a `pub(crate)` struct is `pub(crate)`, never `pub` (proposal 0003) |
| `expr.struct.update` | the base of `..base` is a path, a constructor call or a method call (proposals 0004, 0036) |
| `type.unit` | `()` as a type only in a type argument of a type we don't define, as in `Result<(), E>`, or as `fn main`'s return type (proposals 0005, 0023) |
| `expr.unit` | `()` as a value only as an argument or field value of a fn, method or struct we don't define, as in `Ok(())` (proposals 0005, 0023) |
| `expr.async` | an async block only as a spawn argument or the whole body of an `unfold`, `then` or `use_future` closure (proposal 0006) |
| `attr.tokio::main` | `#[tokio::main]` only on `fn main` (proposal 0006) |
| `expr.loop` | loop must be the whole body of an actor run fn, after at most one `let mut` state (proposal 0007) |
| `expr.loop` | a loop turn takes one input (`select!`, a `match` of `select!`s, or one awaited call) and handles it with one call or `match` (proposal 0007) |
| `expr.break` | `break` only with a value, out of an actor loop (proposal 0007) |
| `item.fn.receiver.mut` | `mut self` only on a fn whose body is an actor loop (proposal 0008) |
| `item.fn.receiver.mut` | `&mut self` only on methods of an actor's loop state, of a type of ours named in one of its field types or in a variant of an enum it owns (proposal 0008), or of a type in `endpoints` (proposal 0035) |
| `stmt.let.mut` | `let mut` only for a loop's state just before the loop, or for a value never reassigned and only lent as `&mut` (proposal 0008) |
| `pat.ident.mut` | a `mut` binding is never reassigned, only lent as `&mut` (proposal 0008) |
| `stmt.let.mut` | `mut` only on a value whose type isn't ours, on an actor's loop state or on a type in `endpoints`; to lend a library field of a value of ours as `&mut`, destructure the value and bind that field `mut` (proposals 0008, 0035) |
| `pat.ident.mut` | `mut` only on a value whose type isn't ours, on an actor's loop state or on a type in `endpoints`; to lend a library field of a value of ours as `&mut`, destructure the value and bind that field `mut` (proposals 0008, 0035) |
| `expr.assign` | assignment only to `self.field` in a `&mut self` method, or to the loop's state binding (proposal 0008) |
| `type.reference.mut` | no `&mut` of our own types other than `&mut self` (proposals 0008, 0023) |
| `expr.call` | `drop(..)` only of a named value that owns a resource released early; discard a value with `let _ = value;` (proposal 0025) |
| `expr.path` | `drop` passed as a function, as in `.map(drop)`, is a discard; write `let _ = value;` (proposal 0025) |
| `expr.method_call` | `.ok()` is not a discard; write `let _ = value;` (proposal 0025) |
| `item.use` | a trait in `fallible_operators` is never imported, named in a path or called as a method; write `checked_*` or `saturating_*` (proposal 0038) |
| `expr.path` | a trait in `fallible_operators` is never imported, named in a path or called as a method; write `checked_*` or `saturating_*` (proposal 0038) |
| `type.path` | a trait in `fallible_operators` is never imported, named in a path or called as a method; write `checked_*` or `saturating_*` (proposal 0038) |
| `expr.method_call` | a trait in `fallible_operators` is never imported, named in a path or called as a method; write `checked_*` or `saturating_*` (proposal 0038) |
| `item.use` | a reader in `consuming_readers` changes its buffer on every read, so a tuple of reads means something only by evaluation order; split the bytes with `split_at_checked` and match them, as in the bootstrap review's §A7 (proposal 0039) |
| `expr.path` | a reader in `consuming_readers` changes its buffer on every read, so a tuple of reads means something only by evaluation order; split the bytes with `split_at_checked` and match them, as in the bootstrap review's §A7 (proposal 0039) |
| `type.path` | a reader in `consuming_readers` changes its buffer on every read, so a tuple of reads means something only by evaluation order; split the bytes with `split_at_checked` and match them, as in the bootstrap review's §A7 (proposal 0039) |
| `macro.matches` | a boolean expression (`matches!`, a comparison, `&&`, `\|\|`, `!`) only as the body of a closure passed straight to a method in `predicate_adapters` (proposals 0028, 0037), or one comparison or `matches!` as a match guard (proposal 0040) |
| `expr.binary.eq` | a boolean expression (`matches!`, a comparison, `&&`, `\|\|`, `!`) only as the body of a closure passed straight to a method in `predicate_adapters` (proposals 0028, 0037), or one comparison or `matches!` as a match guard (proposal 0040) |
| `expr.binary.ne` | a boolean expression (`matches!`, a comparison, `&&`, `\|\|`, `!`) only as the body of a closure passed straight to a method in `predicate_adapters` (proposals 0028, 0037), or one comparison or `matches!` as a match guard (proposal 0040) |
| `expr.binary.lt` | a boolean expression (`matches!`, a comparison, `&&`, `\|\|`, `!`) only as the body of a closure passed straight to a method in `predicate_adapters` (proposals 0028, 0037), or one comparison or `matches!` as a match guard (proposal 0040) |
| `expr.binary.le` | a boolean expression (`matches!`, a comparison, `&&`, `\|\|`, `!`) only as the body of a closure passed straight to a method in `predicate_adapters` (proposals 0028, 0037), or one comparison or `matches!` as a match guard (proposal 0040) |
| `expr.binary.gt` | a boolean expression (`matches!`, a comparison, `&&`, `\|\|`, `!`) only as the body of a closure passed straight to a method in `predicate_adapters` (proposals 0028, 0037), or one comparison or `matches!` as a match guard (proposal 0040) |
| `expr.binary.ge` | a boolean expression (`matches!`, a comparison, `&&`, `\|\|`, `!`) only as the body of a closure passed straight to a method in `predicate_adapters` (proposals 0028, 0037), or one comparison or `matches!` as a match guard (proposal 0040) |
| `expr.binary.and` | a boolean expression (`matches!`, a comparison, `&&`, `\|\|`, `!`) only as the body of a closure passed straight to a method in `predicate_adapters` (proposals 0028, 0037), or one comparison or `matches!` as a match guard (proposal 0040) |
| `expr.binary.or` | a boolean expression (`matches!`, a comparison, `&&`, `\|\|`, `!`) only as the body of a closure passed straight to a method in `predicate_adapters` (proposals 0028, 0037), or one comparison or `matches!` as a match guard (proposal 0040) |
| `expr.unary.not` | a boolean expression (`matches!`, a comparison, `&&`, `\|\|`, `!`) only as the body of a closure passed straight to a method in `predicate_adapters` (proposals 0028, 0037), or one comparison or `matches!` as a match guard (proposal 0040) |
| `expr.method_call` | count matching items with `filter(\|x\| matches!(x, ..)).count()`, not `filter_map(..).count()` (proposal 0028) |
| `expr.match.guard` | a match guard is one comparison (`==`, `!=`, `<`, `<=`, `>`, `>=`) or one `matches!`, with no `&&`, `\|\|` or `!` (proposal 0040) |
| `expr.match.guard` | a guard tests a value its arm's pattern bound; a guard on nothing the pattern bound is an `if` (proposal 0040) |
| `expr.match.guard` | a guard that compares with a literal, a variant or a constant, or a `matches!` on a place, is a pattern: write it in the arm (proposal 0040, bootstrap review §A3, §A4) |
| `expr.match.guard` | a guarded arm needs a later arm with no guard whose pattern covers the guarded one, so the otherwise case is written out (proposal 0040) |
| `macro.matches` | `matches!` takes a place or a call and a pattern with no `if` and no `true` / `false`, so no boolean rides through it (proposals 0028, 0040) |
| `expr.method_call` | `Some(x).filter(..)` or `.find(..)` with one comparison is a match guard: `pat if a == b => ..` with an unguarded arm for the rest (proposal 0040) |
| `expr.method_call` | a `filter` of one comparison matched at once is a match guard: `Some(x) if a == b => ..` with an unguarded arm for the rest (proposal 0040) |
| `expr.method_call` | a comparison by name (`a.eq(&b)`, `a.lt(&b)`, `a.cmp(&b).is_eq()`, `PartialEq::eq`) is a `bool` outside a predicate; write a match guard `pat if a == b => ..` (proposal 0040) |
| `expr.for` | a `for` body holds only calls, as `f(item);`, `f(item)?;`, `x.m(item).await?;` or `let _ = f(item);`, each building nothing; build or select values with the adapters (`map`, `filter`, `fold`, `try_fold`, ..) (proposal 0041) |
| `expr.for` | a `for` iterates a list built before it or by adapters, with no block, assignment or branch in its iterator expression (proposal 0041) |
| `expr.method_call` | a loop whose body performs side effects is `for item in list { .. }`, not `for_each` or `try_for_each` (proposal 0041) |
| `expr.method_call` | awaited effects run in order as `for item in list { f(item).await?; }`, not `stream::iter(..).then(..).try_collect()` into `()` (proposal 0041) |
| `item.impl.trait` | trait impls only for `Drop`, `Debug`, `Serialize`, `Deserialize` and `ProtocolHandler` (proposal 0009) |
| `generics.lifetime` | lifetime parameters only in a library trait impl (proposal 0009) |
| `type.lifetime` | lifetimes only in a library trait impl, where its signature demands them (proposal 0009) |
| `attr.derive` | `thiserror::Error` only on an enum or named struct that is an error type, named `…Error` or used as a `Result`'s error (proposal 0010) |
| `attr.derive` | `PartialOrd` and `Ord` only on a newtype struct whose order means something (proposal 0024) |
| `expr.method_call` | compare an ordered newtype as a whole, `a.cmp(&b)`, never through `.0` (proposal 0024) |
| `attr.error` | each error variant carries one `#[error("…")]` message, never `transparent` (proposal 0010) |
| `attr.from` | `#[from]` only on the one variant of an enum holding a given error type (proposal 0011) |
| `attr.serde` | `#[serde]` only with `transparent`, `tag`, `content`, `rename_all` and `rename` (proposal 0012) |
| `generics.type_param` | a type parameter only on a fn, with exactly one bound, `Serialize` or `DeserializeOwned`, and no `where` (proposal 0013), or as the unbounded `T` of an `ask` helper (proposal 0026) |
| `type.impl_trait.arg` | `impl Trait` as an argument only as `build: impl FnOnce(oneshot::Sender<T>) -> Msg`, the last parameter of a private `ask<T>` helper (proposal 0026) |
| `type.slice` | `&[T]` only as a parameter type or a `const` type; a `const` list is `const NAME: &[T] = &[..]` (proposal 0014) |
| `type.array` | an array type only for bytes whose size is part of a format or API (`[u8; N]`); a `const` list is `&[T]` (proposals 0014, 0015) |
| `expr.array` | an array literal only passed straight to a call, as in `f([a, b])` or `f(&[a, b])`, or as a `const` value (proposal 0015) |
| `expr.repeat` | an array repeat only of a literal, as in `[0u8; N]` (proposal 0015) |
| `attr.cfg` | `cfg` only with `target_family = "wasm"`, `unix` or their `not` (proposal 0016) |
| `attr.cfg` | `cfg` only on a mod, use, fn or impl item, or as `#![cfg]` atop a test file (proposal 0016) |
| `attr.cfg` | a `cfg` fn, impl or use needs its counterpart for the other platform in the same file (proposal 0016) |
| `type.dyn` | `dyn` only as `Closure<dyn FnMut(..)>` (proposal 0017) |
| `expr.closure.adapter` | a closure passed to a method only to a method in `closure_methods` or `predicate_adapters`: the iterator, `Option` and `Result` methods and the library methods the config lists (proposals 0006, 0026, 0028, 0037) |
| `expr.closure` | a closure outside a method call only as the argument of `Closure::new`, `unfold`, `ask` or a Dioxus hook (proposals 0006, 0017, 0018, 0026) |
| `item.use.glob` | a glob import only for `dioxus::prelude::*` (proposal 0018) |
| `attr.allow` | only `#![allow(clippy::useless_format)]` at the crate root (proposal 0018) |
| `type.bool` | `bool` only in a type argument of a type we don't define, or as a flag field of a clap `Parser` struct (proposals 0019, 0023) |
| `attr.arg` | a choice between named values is a `ValueEnum`, not `value_parser = [...]` (proposal 0019) |
| `attr.tokio::test` | `#[tokio::test]` takes no arguments and marks an `async fn` (proposal 0020) |
| `expr.lit.bool` | `true` / `false` only as an argument or field value of a fn, method or struct we don't define (proposal 0023) |
| `macro.env` | `env!` only for `CARGO_PKG_VERSION`, `CARGO_PKG_NAME` and `CARGO_MANIFEST_DIR` (proposal 0022) |

## How constructs are classified

Every item, expression, statement, pattern, type, generic parameter,
attribute and macro invocation gets exactly one id. Structural nodes such as
blocks, match arms, fields, paths, fn parameters and signatures carry no id
of their own. Their modifiers do. For example, `async` on a fn is
`item.fn.async`, reported at the `async` keyword, and `&mut self` is
`item.fn.receiver.mut`. A syn node kind the tool doesn't know is reported
as `unknown.<kind>`, which is never allowed, so new syntax can't pass by
accident.

Some rules need context.

- `expr.closure.adapter` is a closure written directly as an argument of a
  method call. Any other closure is `expr.closure`.
- `stmt.let.mut` is a `let` with any `mut` binding in its pattern. Those
  bindings aren't reported again as `pat.ident.mut`, which covers `mut`
  bindings everywhere else, such as fn parameters and match arms.
- `expr.if_let.single` is an `if let` with no `else`. An `if let` with an
  `else` or `else if`, and an `if` whose condition is a let-chain, are
  `expr.if_let`. The `let`s of a chain are also `expr.let` and its `&&` is
  `expr.binary.and`.
- `type.impl_trait.ret` is `impl Trait` anywhere in a fn's return type, and
  `type.impl_trait.arg` is `impl Trait` anywhere else.
- `attr.derive` is reported when every derived name is in `derives`.
  Otherwise each unlisted name is reported as `attr.derive.<Name>`, using
  the path as written, such as `attr.derive.serde::Serialize`.
- `macro.allowed` is reported when the macro's full path (such as
  `tracing::info`) or its last segment (such as `format`) is in `macros`.
  Inside a `#[cfg(test)]` item, a `#[test]` or `#[tokio::test]` fn, or a
  file with a `tests` directory in its path, `test_macros` count too. Any
  other macro is `macro.<path>`, such as `macro.println` or
  `macro.tracing::warn`. `env!` of anything but `CARGO_PKG_VERSION`,
  `CARGO_PKG_NAME` or `CARGO_MANIFEST_DIR` is `macro.env`, whatever
  `macros` lists (0022).
- `attr.cfg_test` covers `#[test]` and exactly `#[cfg(test)]`. Other `cfg`
  attributes are `attr.cfg`.
- A path type or expression path that names `Cell`, `RefCell`, `OnceCell`,
  `OnceLock`, `UnsafeCell`, `Mutex`, `RwLock` or an `Atomic*` type in any
  segment is `type.interior_mut.<Name>` or `expr.interior_mut.<Name>`, such
  as `type.interior_mut.RefCell` inside `Rc<RefCell<T>>` or
  `expr.interior_mut.AtomicBool` for `AtomicBool::new`.

## Measuring

- **Tokens** are `proc_macro2` token leaves: each identifier, punctuation
  character and literal counts as one, and each delimited group (`()`,
  `[]`, `{}`) counts as one for its pair of delimiters. A multi-character
  operator such as `::` or `=>` is two tokens. Comments and doc comments
  don't count.
- **Lines** are the lines that hold at least one counted token. Blank lines,
  comment lines and doc-comment lines don't count. A string literal that
  spans several lines counts every line it covers.

## The weighted score

A line count treats every line alike. The weighted score charges each token
by what kind of code it is part of, so a codebase that leans on the
constructs it wants to encourage scores lower than one of the same size
that leans on compromises. It is a better measure of code volume for
comparing two versions of the same code.

### The model

- **Tokens** are the ones `measure` counts. Comments and doc comments cost
  nothing.
- **Every token belongs to the innermost construct that contains it**: the
  smallest syntax node with an id whose span covers the token. In
  `match x { A => f(y), }`, the `match`, its braces, the `=>` and the `,`
  belong to the `match`, `A` to its pattern, and `f`, `(..)` and `y` to the
  call and its paths. A token outside every construct is `(no construct)`
  and costs the neutral weight.
- **Some nodes fold their insides into themselves.** The whole head of an
  `if let` with no `else` (`if let Pat = expr`) costs the `if let`'s
  weight, and the body's tokens belong to whatever is inside it. A type
  that is primitive or untyped (see Typing) costs its own weight for all of
  its tokens, so `Vec<u8>` is one primitive, not a `Vec` and a `u8`. A
  violation inside a folded node still costs its own weight.
- **A token costs the weight of its construct's tier.** A construct that
  breaks the subset costs `outside`, whatever its id. A construct whose
  shape an item we don't define fixes costs `boundary` (below). Any other
  construct costs the tier `[weights.ids]` gives its id.
- **The score of a fn, file or crate** is the sum over its tokens. Its
  score per token is the score divided by its tokens.

### Configuration

`score` reads the `[weights]` table of the config:

```toml
[weights]
external_protocols = ["src/vendor_api/**"]   # optional, default none
untyped = ["serde_json::Value", "dyn Any"]  # optional, default below
# adapters, primitives, transparent         # optional, default below

[weights.tiers]
encouraged = 0.5
neutral = 1.0
tolerated = 2.0
costly = 3.0
outside = 5.0
boundary = 1.0

[weights.ids]
"item.enum" = "encouraged"
"expr.loop" = "costly"
```

- `tiers` gives each tier its weight. All six are required.
- `ids` maps an id to `encouraged`, `neutral`, `tolerated`, `costly`,
  `outside` or `boundary`. An allowed id missing from `ids` is neutral. A
  violation is always `outside`. List every allowed id explicitly: a
  project can keep a test that fails when a newly allowed id has no tier.
- `adapters` lists method names whose calls are
  `expr.method_call.adapter`, so that iterator adapters can take their own
  tier. The default is the std `Iterator` methods: `all`, `any`, `chain`,
  `cloned`, `collect`, `copied`, `count`, `cycle`, `enumerate`, `filter`,
  `filter_map`, `find`, `find_map`, `flat_map`, `flatten`, `fold`, `fuse`,
  `inspect`, `last`, `map`, `map_while`, `max`, `max_by`, `max_by_key`,
  `min`, `min_by`, `min_by_key`, `nth`, `partition`, `peekable`,
  `position`, `product`, `reduce`, `rev`, `rposition`, `scan`, `skip`,
  `skip_while`, `step_by`, `sum`, `take`, `take_while`, `try_fold`, `unzip`
  and `zip`. The name is all the tool sees, so `Option::map` counts too.
- `primitives`, `untyped`, `transparent` and `external_protocols` are
  described under Typing.

A list that is given replaces its default. To extend a default, copy it.

### Score-only ids

The score refines a few allowed ids by where they stand, so they can have
their own tier. Give these tiers in `ids` like any other id.

| Id | Meaning |
|---|---|
| `expr.method_call.adapter` | method call whose name is in `adapters` |
| `stmt.let.discard` | `let _ = value;` |
| `type.field` | type in a field of our own struct or enum variant |
| `type.primitive` | type in `primitives`, as a field type or in a signature of our own |
| `type.untyped` | type in `untyped`, anywhere |
| `field.wire` | field of a wire type in an external protocol module |

`field.wire` always costs the boundary weight.

### The boundary context

Some shapes exist only because a library demands them: a trait impl the
library calls, a `bool` its function takes, a `let mut` for a value its
method borrows as `&mut`. Charging those as style debt would punish code
for the libraries it uses. So **a construct whose shape an item we don't
define fixes costs the `boundary` weight instead of its tier**, wherever it
is allowed. "We don't define" uses the same facts as the shape rules: a
path is ours when it starts from `crate`, `self`, `super`, one of our
crates or modules, or names a type or fn we define, and a method call is
ours only when its receiver is `self`.

The tool applies that one standard in four positions:

1. **An argument, field value or type argument of an item we don't
   define.** Such as `channel(false)`, `Some(())`, `Closure<dyn FnMut()>`
   or `spawn(async move { .. })`. Here the boundary relieves only a
   construct whose tier is above neutral (tolerated or costly), since any
   construct can stand in an argument and a plain path or call there is not
   library glue.
2. **An impl of a trait we don't define**, with the generics, lifetimes and
   `&mut self` receivers its signatures fix, and a generic parameter whose
   bounds are all traits we don't define.
3. **A `mut` binding that exists to be lent as `&mut`**: a `let mut` or
   `mut` parameter other than an actor loop's state, and a `&mut T` type of
   a `T` we don't define. An actor loop's own `let mut` state, our own
   `&mut self` methods and assignments keep their tier.
4. **Library syntax**: an attribute other than the language's own (`cfg`,
   `derive`, `doc`, `test`, the lint attributes and so on), a derive of
   anything outside the std derives (`Debug`, `Clone`, `Copy`, `PartialEq`,
   `Eq`, `Hash`, `PartialOrd`, `Ord`, `Default`), and a macro outside std
   (`format!`, `vec!`, `matches!`, `assert_eq!` and the rest), with
   everything inside its delimiters.

A violation is never boundary. `score` reports the boundary score and its
share at every level, so library glue is visible and isn't read as style
debt.

### Typing

The score also rewards defining types for data.

- **Type definitions are cheap.** Give `item.struct.named`,
  `item.struct.newtype`, `item.enum` and `type.field` a low tier. Every
  type in a field of our own struct or enum variant is `type.field`, except
  primitive and untyped ones. A newtype's one field is always `type.field`,
  since wrapping a primitive is what a newtype is for.
- **Primitives in our own signatures and fields cost more.** A type that
  matches a `primitives` pattern is `type.primitive` when it is a field
  type of our own struct or variant, or a parameter or return type of our
  own fn (not of an impl of a trait we don't define). In fn bodies a
  primitive is an ordinary `type.path`. A reference to a primitive, such as
  `&str`, is primitive. The default `primitives` are `String`, `str`,
  `char`, the integer and float types, and `Vec<u8>`.
- **Untyped data costs more everywhere.** A type that matches an `untyped`
  pattern is `type.untyped`, in a signature, a field or a body. The
  default `untyped` are `HashMap<String, _>`, `BTreeMap<String, _>` and
  `dyn Any`. A project adds its JSON value type, such as
  `serde_json::Value`.
- **Wire types of an external protocol are boundary.** In a file matched by
  an `external_protocols` glob, each field of a struct or enum with a
  non-std derive (a serialization derive, say) is `field.wire`, and every
  construct inside it costs the boundary weight. Use it for modules that
  mirror a protocol someone else defines. A protocol of your own keeps the
  normal rules.

A type pattern is Rust type syntax. `_` matches any type, a path matches on
its last segment and on the generic arguments the pattern gives, and a
pattern's leading segments must agree where the code spells them too. A
bare name in the code, such as `Value`, matches `serde_json::Value` unless
it is a name we define.

- **Std containers are transparent.** A type whose last segment is in
  `transparent`, and isn't a name we define, holds its arguments without
  typing them. A primitive held only by such containers in our own field or
  signature, as `String` in `Option<String>` or `Vec<Option<String>>`, is
  `type.primitive` at its own tier, not boundary. The container's own
  tokens are left out of the typing shares, so `Option<SessionId>` counts
  as our own type. The default `transparent` is `Option`, `Result`, `Vec`,
  `VecDeque`, `Box`, `Rc`, `Arc`, `Cow`, `HashMap`, `HashSet`, `BTreeMap`
  and `BTreeSet`.

Positions 1 to 3 of the boundary context apply to these ids too: a
primitive passed as a type argument of a library type that isn't
transparent, as in `mpsc::Sender<String>`, costs the boundary weight.

## The HTML report

`subset report-html --out <dir>` writes one self-contained page,
`<dir>/index.html`: HTML with inline SVG charts and no JavaScript. Hovering
a mark shows its numbers in a native tooltip, and every chart has a table
with the same numbers. The page follows the system's light or dark mode,
and each chart is drawn twice, for wide and narrow screens, so its text
stays legible on a phone.

The first group is the files under `include`. Each `--compare <dir>` adds
a group of every `*.rs` file under that directory, scored under the same
config and labelled as a comparison, for code outside the subset such as
the checker itself or vendored crates. For each group the page shows:

1. **Tiles**: the score, the score in lines, tokens and lines, score per
   token, violations and boundary share.
2. **Score per crate, by tier**: a stacked bar per crate, with the
   boundary score as its own segment, and the score in lines beside it on
   wide screens.
3. **Files**: a treemap with each file's area set by its score and its
   colour by its score per token, so concentrations of compromise stand
   out. The tooltip gives the file's boundary share and violations.
4. **Typing**: per crate, the share of field and signature type tokens
   that are our own named types, primitives, untyped data, or other types,
   and the 20 fns with the most primitive tokens in their signatures, as
   candidates for newtypes.
5. **Constructs**: each id's count, tokens, tier, weight and share of the
   score, sorted by contribution.
6. **Functions**: the top 20 by score, and by score per token among fns
   of at least 30 tokens, with `file:line`.

The **score in lines** is approximate: the score divided by the tokens per
line of the scope shown, `score ÷ (tokens ÷ lines)`, for a sense of scale.
Wherever the page gives tokens, in tiles, tooltips and tables, it gives
lines beside them, except the typing shares, which count type tokens.

`--history <n>` adds a line chart of the score per crate at each of the
last `n` commits that changed files under `include`, read with `git` and
scored under the current config. The config must sit at the root of a git
repository with that history, so a shallow clone needs a full fetch. A
commit whose files don't parse is left out, with a note on stderr.

## Known limits

- **Macro bodies aren't checked.** The tool reports the macro's id and
  doesn't look inside it. Code in `format!` arguments, `tracing` fields or
  Dioxus `rsx!` can use any construct.
- **`expr.closure.adapter` can't see types.** syn has no type information,
  so the rule checks the method's name against `closure_methods` and
  `predicate_adapters`, not that the receiver is an iterator, `Option` or
  `Result`. A method of ours that shares a listed name, such as a `map` of
  our own, passes.
- **`type.bool` matches the path `bool` only.** A type alias for `bool`, or
  a `bool` returned by a std method, isn't detected.
- **Attribute arguments are only partly read.** The rules read the keys of
  `#[serde(...)]`, `value_parser = [` in `#[arg(...)]`, the message of
  `#[error(...)]` and the predicate of `#[cfg(...)]`. Other attributes are
  one id, whatever they contain.
- **Shape rules can't see types.** "Ours" means a path from `crate`, `self`,
  `super`, one of our crates or modules, a name we define, or an associated
  item of a type we define. So 0008(b) `let mut` for an external value is
  checked as "never reassigned", 0023's library boundary as "not ours", and
  a method's receiver is ours only when it is `self`. A library type that
  shares a name with one of ours counts as ours, and a library wrapper hides
  where a value goes: `Option<bool>` as our field and `Some(true)` as its
  value pass, because `Option` and `Some` aren't ours.
- **Some rules stay with the reviewer.** Syntax can't tell why a construct
  is there: 0002 `pub` only for another crate, 0006 `async fn` only for I/O,
  0012 serde only on wire types (checked by location), 0013 generics only at
  the wire boundary (checked by bound), 0014 a slice only for a read-only
  sequence, 0015 a byte array only where its size is part of a format, 0009
  generics only where the trait demands them (any are allowed inside a
  listed trait impl), 0024 an order that means something, 0025 `drop` only
  of a value that owns a resource (checked as "the argument is a plain
  name", and `drop` passed as a function is flagged), 0026 every round trip
  going through `ask` (the helper's shape is checked), 0036 a struct
  update's method-call base returning the struct's own type (any method call
  passes), and 0028 a `fold` that only counts (`filter_map(..).count()` is
  checked). Neither can it see what a body does: 0017's callback only
  forwards to a channel, and 0019's `main` turns its `bool` into an enum.
  `docs/subset/compromises.md` lists these.
- **Ownership and receivers are approximated by name.** 0008's "a value
  that loop state owns" is a type of ours named anywhere in a field type of
  a loop-state type (`Vec<Turn>`, `Option<FrameBuffer>`), one step down.
  An enum of ours is transparent, as `Option` is: the types in its
  variants' payloads are owned wherever the enum is, so `Connected` in
  `phase: Phase` with `Phase::Connected(Connected)` is owned. A type that a
  loop state merely mentions, say in `PhantomData<T>`, counts too, and a
  value two struct fields down doesn't. 0024's `.0.cmp(` is flagged
  only where the receiver is `self` in an impl of an ordered newtype or a
  parameter declared as one; `event.seq.0.cmp(..)` passes.
- **`cfg` pairs are matched by name.** A `cfg` fn or impl is paired when an
  item of the same kind and name has the other platform's predicate. Any
  `use` pairs with any `use`, and a `cfg` module needs no pair, since 0016
  exempts modules that exist on one platform only.
- **A guard's fallback arm is matched by pattern structure.** 0040 wants a
  later arm with no guard whose pattern covers the guarded one. The check
  compares the two patterns: `_` and a lowercase binding cover anything, a
  tuple, tuple struct or struct covers field by field under the same path,
  an or-pattern covers when one of its cases does, and anything else covers
  only when it is written the same way. A fallback split across several
  arms, or one behind `..` in a tuple, isn't seen and is flagged.
- **0040's guard is checked by syntax.** A guard must have an operand, or
  a `matches!` scrutinee, that is a place or call chain rooted in a name
  the arm's pattern binds. A comparison operand that is a literal, an
  uppercase path (a unit variant or a constant) or a constructor of such
  values, and a `matches!` scrutinee with no call in it, are patterns and
  are flagged. The checker can't see types, so these are left to review:
  `Some(v) if v.is_empty() == w.is_empty()`, two library booleans compared
  under a bound name; a constant written in lowercase; and `match
  a.cmp(&b)` with only `Ordering::Equal` and a catch-all, an equality
  written as an ordering.
- **Lifts are matched by name and shape.** `eq`, `ne`, `lt`, `le`, `gt` and
  `ge` with one argument, an `Ordering` test on `cmp` or `partial_cmp`, and
  `PartialEq::eq`, `PartialOrd::lt` and the like as paths are flagged
  anywhere outside a predicate closure, so a method of ours with one of
  those names is flagged too. `filter` or `find` with one comparison (under
  any `!`) is flagged on `Some(..)` by any path, on `Some(..).iter()` or
  `.into_iter()`, on a name bound with `let x = Some(..)` anywhere in the
  same fn, and on any receiver when the call is a `match` or `if let`
  scrutinee. A filter result bound first and matched later, or an `Option`
  built through a helper fn, is left to review.
- **`matches!` is parsed.** In a guard or a predicate closure, its
  arguments must parse as a scrutinee, a pattern and no `if`. The
  scrutinee is a place, a call, a tuple of these or a closure argument of
  one, with no operator or block, and the pattern holds no `true` or
  `false`. A macro that doesn't parse that way is flagged.
- **0041 recognises effects by shape.** A `for` body is checked statement
  by statement: a call, then optionally `.await` and `?`, or `let _ =` a
  call. The call, its receiver and its arguments, and the `for`'s iterator
  expression, may hold no block, assignment, `let`, branch or loop outside
  the closures passed to adapters, and no assignment inside them; the call
  holds no second `.await`. `for_each` and `try_for_each` are flagged by
  name. A `try_collect` on a chain that starts at `stream::iter`, a bare
  `iter(..)` or a name bound to such a chain in the same fn is flagged
  unless its turbofish names a list type with no `()` in it. A `collect`
  into `Vec<Result<(), E>>` that is then checked for errors runs effects
  too, but syntax can't tell it from a list of results that is kept, so it
  is a review check, as is a stream built by a helper fn.
- **`pat.wild` is allowed everywhere.** The rule in `subset.toml` that
  limits `_` to the positions clippy's wildcard lints allow is left to
  clippy.
- **Files must parse with syn 2.** A file that doesn't parse stops `check`
  with exit code 2.

## Construct ids

Every id the tool can emit. The last five rows are families whose final
part comes from the source.

| Id | Meaning |
|---|---|
| `item.struct.named` | struct with named fields |
| `item.struct.newtype` | struct with exactly one unnamed field |
| `item.struct.tuple` | tuple struct with zero or several unnamed fields |
| `item.struct.unit` | unit struct |
| `item.enum` | enum |
| `item.enum.discriminant` | explicit enum discriminant |
| `item.fn` | function or method |
| `item.fn.async` | async fn |
| `item.fn.const` | const fn |
| `item.fn.unsafe` | unsafe fn |
| `item.fn.extern` | fn with an explicit ABI |
| `item.fn.variadic` | variadic fn |
| `item.fn.receiver.mut` | `mut self` or `&mut self` receiver |
| `item.fn.receiver.typed` | self receiver with an explicit type |
| `item.impl.inherent` | inherent impl |
| `item.impl.trait` | trait impl |
| `item.impl.unsafe` | unsafe impl |
| `item.impl.type` | associated type in an impl |
| `item.const` | const item |
| `item.static` | static item |
| `item.static.mut` | static mut item |
| `item.use` | use declaration |
| `item.use.glob` | glob import |
| `item.use.rename` | import renamed with `as` |
| `item.mod` | module |
| `item.extern_crate` | extern crate |
| `item.foreign_mod` | extern block |
| `item.foreign.fn` | fn in an extern block |
| `item.foreign.static` | static in an extern block |
| `item.foreign.type` | type in an extern block |
| `item.trait` | trait definition |
| `item.trait.unsafe` | unsafe trait |
| `item.trait.auto` | auto trait |
| `item.trait.const` | associated const in a trait |
| `item.trait.fn` | fn in a trait |
| `item.trait.type` | associated type in a trait |
| `item.trait_alias` | trait alias |
| `item.type` | type alias |
| `item.union` | union |
| `item.macro_rules` | macro_rules! definition |
| `vis.pub` | `pub` visibility |
| `vis.restricted` | `pub(crate)`, `pub(super)` or `pub(in path)` visibility |
| `expr.array` | array expression |
| `expr.assign` | assignment |
| `expr.async` | async block |
| `expr.await` | `.await` |
| `expr.binary.add` | `+` operator |
| `expr.binary.sub` | `-` operator |
| `expr.binary.mul` | `*` operator |
| `expr.binary.div` | `/` operator |
| `expr.binary.rem` | `%` operator |
| `expr.binary.and` | `&&` operator |
| `expr.binary.or` | `\|\|` operator |
| `expr.binary.bit_xor` | `^` operator |
| `expr.binary.bit_and` | `&` operator |
| `expr.binary.bit_or` | `\|` operator |
| `expr.binary.shl` | `<<` operator |
| `expr.binary.shr` | `>>` operator |
| `expr.binary.eq` | `==` operator |
| `expr.binary.lt` | `<` operator |
| `expr.binary.le` | `<=` operator |
| `expr.binary.ne` | `!=` operator |
| `expr.binary.ge` | `>=` operator |
| `expr.binary.gt` | `>` operator |
| `expr.binary.add_assign` | `+=` operator |
| `expr.binary.sub_assign` | `-=` operator |
| `expr.binary.mul_assign` | `*=` operator |
| `expr.binary.div_assign` | `/=` operator |
| `expr.binary.rem_assign` | `%=` operator |
| `expr.binary.bit_xor_assign` | `^=` operator |
| `expr.binary.bit_and_assign` | `&=` operator |
| `expr.binary.bit_or_assign` | `\|=` operator |
| `expr.binary.shl_assign` | `<<=` operator |
| `expr.binary.shr_assign` | `>>=` operator |
| `expr.block` | block expression |
| `expr.block.labeled` | labeled block |
| `expr.break` | `break` |
| `expr.call` | function call |
| `expr.cast` | `as` cast |
| `expr.closure` | closure that is not a direct argument of a method call |
| `expr.closure.adapter` | closure passed straight to a method call |
| `expr.closure.async` | async closure |
| `expr.closure.const` | const closure |
| `expr.closure.static` | static closure |
| `expr.const_block` | const block |
| `expr.continue` | `continue` |
| `expr.field` | field access |
| `expr.for` | `for` loop |
| `expr.group` | invisible group expression |
| `expr.if` | `if` expression |
| `expr.if_let` | `if let` with an `else`, or an `if` with a let-chain |
| `expr.if_let.single` | `if let` with no `else` |
| `expr.index` | indexing |
| `expr.infer` | `_` in expression position |
| `expr.let` | `let` inside a condition chain |
| `expr.lit` | literal other than `true` / `false` |
| `expr.lit.bool` | `true` / `false` literal |
| `expr.loop` | `loop` |
| `expr.match` | `match` expression |
| `expr.match.guard` | `if` guard on a match arm |
| `expr.method_call` | method call |
| `expr.paren` | parenthesized expression |
| `expr.path` | path expression |
| `expr.path.qualified` | qualified path `<T as Trait>::item` |
| `expr.range` | range expression |
| `expr.raw_addr` | `&raw` pointer |
| `expr.reference` | `&x` |
| `expr.reference.mut` | `&mut x` |
| `expr.repeat` | array repeat `[x; n]` |
| `expr.return` | `return` |
| `expr.struct` | struct or enum-variant construction |
| `expr.struct.update` | struct construction with `..base` |
| `expr.try` | `?` operator |
| `expr.try_block` | `try` block |
| `expr.tuple` | tuple expression |
| `expr.unit` | `()` value |
| `expr.unary.deref` | `*` dereference |
| `expr.unary.not` | `!` operator |
| `expr.unary.neg` | unary `-` |
| `expr.unsafe` | unsafe block |
| `expr.while` | `while` loop |
| `expr.while_let` | `while let` loop |
| `expr.yield` | `yield` |
| `stmt.let` | immutable `let` with an initializer |
| `stmt.let.mut` | `let` with a `mut` binding |
| `stmt.let.else` | `let ... else` |
| `stmt.let.uninit` | `let` without an initializer |
| `pat.const` | const block pattern |
| `pat.ident` | binding pattern |
| `pat.ident.mut` | `mut` binding outside `let` |
| `pat.ident.ref` | `ref` binding |
| `pat.ident.ref_mut` | `ref mut` binding |
| `pat.ident.at` | `name @ pattern` binding |
| `pat.lit` | literal pattern other than `true` / `false` |
| `pat.lit.bool` | `true` / `false` pattern |
| `pat.or` | `A \| B` pattern |
| `pat.paren` | parenthesized pattern |
| `pat.path` | path pattern (unit variant or const) |
| `pat.range` | range pattern |
| `pat.reference` | `&pat` pattern |
| `pat.rest` | `..` pattern |
| `pat.slice` | slice pattern |
| `pat.struct` | struct pattern |
| `pat.tuple` | tuple pattern |
| `pat.tuple_struct` | tuple-struct pattern |
| `pat.type` | type ascription in `let` or a closure parameter |
| `pat.wild` | `_` pattern |
| `type.array` | array type |
| `type.bool` | `bool` type |
| `type.fn_ptr` | fn pointer type |
| `type.group` | invisible group type |
| `type.impl_trait.arg` | `impl Trait` outside a return type |
| `type.impl_trait.ret` | `impl Trait` in a return type |
| `type.infer` | `_` type |
| `type.never` | `!` type |
| `type.paren` | parenthesized type |
| `type.path` | path type other than `bool` |
| `type.path.qualified` | qualified path type `<T as Trait>::Assoc` |
| `type.ptr` | raw pointer type |
| `type.reference` | `&T` |
| `type.reference.mut` | `&mut T` |
| `type.slice` | slice type |
| `type.dyn` | trait object |
| `type.tuple` | tuple type |
| `type.unit` | `()` type |
| `type.lifetime` | lifetime in a type or bound |
| `generics.type_param` | generic type parameter |
| `generics.lifetime` | lifetime parameter |
| `generics.const_param` | const generic parameter |
| `generics.where` | where clause |
| `generics.hrtb` | `for<'a>` bound |
| `generics.assoc_binding` | associated item binding `Trait<Item = T>` |
| `generics.const_arg` | const generic argument |
| `attr.doc` | doc comment or `#[doc]` |
| `attr.derive` | derive of names listed in `derives` |
| `attr.cfg_test` | `#[cfg(test)]` or `#[test]` |
| `macro.allowed` | macro listed in `macros` (or `test_macros` in tests) |
| `unknown.item` | item kind the classifier does not know |
| `unknown.impl_item` | impl item kind the classifier does not know |
| `unknown.trait_item` | trait item kind the classifier does not know |
| `unknown.foreign_item` | extern block item kind the classifier does not know |
| `unknown.expr` | expression kind the classifier does not know |
| `unknown.pat` | pattern kind the classifier does not know |
| `unknown.type` | type kind the classifier does not know |
| `unknown.bound` | bound kind the classifier does not know |
| `unknown.generic_arg` | generic argument kind the classifier does not know |
| `unknown.attr` | attribute the classifier cannot read |
| `attr.derive.<Name>` | derive of a name not in `derives` |
| `attr.<name>` | attribute not in the subset |
| `macro.<path>` | macro not listed in `macros` |
| `type.interior_mut.<Name>` | std cell, lock or atomic type |
| `expr.interior_mut.<Name>` | std cell, lock or atomic type in an expression path |
