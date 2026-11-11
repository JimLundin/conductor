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
  score per token, the boundary score and the boundary's share of the
  score. Fns and constructs are sorted by score, highest first. `--json`
  prints `{"by", "rows": [{"name", "score", "tokens", "per_token",
  "boundary", "boundary_share"}], "total"}`, with `count` and `tier` on
  construct rows. See "The weighted score" below.
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
are names, not resolved paths.

| Id | Rule |
|---|---|
| `vis.restricted` | only `pub(crate)`, never `pub(super)` or `pub(in …)` (proposal 0003) |
| `vis.pub` | a field of a `pub(crate)` struct is `pub(crate)`, never `pub` (proposal 0003) |
| `expr.struct.update` | the base of `..base` is a path or a constructor call (proposal 0004) |
| `type.unit` | `()` as a type only in a type argument of a type we don't define, as in `Result<(), E>`, or as `fn main`'s return type (proposals 0005, 0023) |
| `expr.unit` | `()` as a value only as an argument or field value of a fn, method or struct we don't define, as in `Ok(())` (proposals 0005, 0023) |
| `expr.async` | an async block only as a spawn argument or the whole body of an `unfold`, `then` or `use_future` closure (proposal 0006) |
| `attr.tokio::main` | `#[tokio::main]` only on `fn main` (proposal 0006) |
| `expr.loop` | loop must be the whole body of an actor run fn, after at most one `let mut` state (proposal 0007) |
| `expr.loop` | a loop turn takes one input (`select!`, a `match` of `select!`s, or one awaited call) and handles it with one call or `match` (proposal 0007) |
| `expr.break` | `break` only with a value, out of an actor loop (proposal 0007) |
| `item.fn.receiver.mut` | `mut self` only on a fn whose body is an actor loop (proposal 0008) |
| `item.fn.receiver.mut` | `&mut self` only on methods of an actor's loop state or of a type of ours named in one of its field types or in a variant of an enum it owns (proposal 0008) |
| `stmt.let.mut` | `let mut` only for a loop's state just before the loop, or for a value never reassigned and only lent as `&mut` (proposal 0008) |
| `pat.ident.mut` | a `mut` binding is never reassigned, only lent as `&mut` (proposal 0008) |
| `expr.assign` | assignment only to `self.field` in a `&mut self` method, or to the loop's state binding (proposal 0008) |
| `type.reference.mut` | no `&mut` of our own types other than `&mut self` (proposals 0008, 0023) |
| `expr.call` | `drop(..)` only of a named value that owns a resource released early; discard a value with `let _ = value;` (proposal 0025) |
| `expr.path` | `drop` passed as a function, as in `.map(drop)`, is a discard; write `let _ = value;` (proposal 0025) |
| `expr.method_call` | `.ok()` is not a discard; write `let _ = value;` (proposal 0025) |
| `macro.matches` | a boolean expression (`matches!`, a comparison, `&&`, `\|\|`, `!`) only as the body of a closure passed straight to `filter`, `find`, `any`, `all`, `position`, `take_while`, `skip_while` or `retain` (proposal 0028) |
| `expr.binary.eq` | a boolean expression (`matches!`, a comparison, `&&`, `\|\|`, `!`) only as the body of a closure passed straight to `filter`, `find`, `any`, `all`, `position`, `take_while`, `skip_while` or `retain` (proposal 0028) |
| `expr.binary.ne` | a boolean expression (`matches!`, a comparison, `&&`, `\|\|`, `!`) only as the body of a closure passed straight to `filter`, `find`, `any`, `all`, `position`, `take_while`, `skip_while` or `retain` (proposal 0028) |
| `expr.binary.lt` | a boolean expression (`matches!`, a comparison, `&&`, `\|\|`, `!`) only as the body of a closure passed straight to `filter`, `find`, `any`, `all`, `position`, `take_while`, `skip_while` or `retain` (proposal 0028) |
| `expr.binary.le` | a boolean expression (`matches!`, a comparison, `&&`, `\|\|`, `!`) only as the body of a closure passed straight to `filter`, `find`, `any`, `all`, `position`, `take_while`, `skip_while` or `retain` (proposal 0028) |
| `expr.binary.gt` | a boolean expression (`matches!`, a comparison, `&&`, `\|\|`, `!`) only as the body of a closure passed straight to `filter`, `find`, `any`, `all`, `position`, `take_while`, `skip_while` or `retain` (proposal 0028) |
| `expr.binary.ge` | a boolean expression (`matches!`, a comparison, `&&`, `\|\|`, `!`) only as the body of a closure passed straight to `filter`, `find`, `any`, `all`, `position`, `take_while`, `skip_while` or `retain` (proposal 0028) |
| `expr.binary.and` | a boolean expression (`matches!`, a comparison, `&&`, `\|\|`, `!`) only as the body of a closure passed straight to `filter`, `find`, `any`, `all`, `position`, `take_while`, `skip_while` or `retain` (proposal 0028) |
| `expr.binary.or` | a boolean expression (`matches!`, a comparison, `&&`, `\|\|`, `!`) only as the body of a closure passed straight to `filter`, `find`, `any`, `all`, `position`, `take_while`, `skip_while` or `retain` (proposal 0028) |
| `expr.unary.not` | a boolean expression (`matches!`, a comparison, `&&`, `\|\|`, `!`) only as the body of a closure passed straight to `filter`, `find`, `any`, `all`, `position`, `take_while`, `skip_while` or `retain` (proposal 0028) |
| `expr.method_call` | count matching items with `filter(\|x\| matches!(x, ..)).count()`, not `filter_map(..).count()` (proposal 0028) |
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
# adapters = [...], primitives = [...]      # optional, default below

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
- `primitives`, `untyped` and `external_protocols` are described under
  Typing.

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

Positions 1 to 3 of the boundary context apply to these ids too: a
primitive passed as a type argument of a library type, as in
`Result<String, E>` or `Option<String>`, costs the boundary weight.

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

1. **Tiles**: the score, tokens, score per token, violations and boundary
   share.
2. **Score per crate, by tier**: a stacked bar per crate, with the
   boundary score as its own segment.
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
  so the rule can't verify that the receiver is an iterator, `Option` or
  `Result`. A closure passed to any method passes.
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
  sequence, 0015 a byte array only where its size is part of a format,
  0009 generics only where the trait demands them (any are allowed inside a
  listed trait impl), 0024 an order that means something, 0025 `drop` only
  of a value that owns a resource (checked as "the argument is a plain
  name", and `drop` passed as a function is flagged), 0026 every round trip going through `ask` (the helper's shape is
  checked), and 0028 a `fold` that only counts (`filter_map(..).count()` is
  checked). Neither can it see what a body does: 0017's callback
  only forwards to a channel, and 0019's `main` turns its `bool` into an
  enum. `docs/subset/compromises.md` lists these.
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
