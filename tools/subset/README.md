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
subset ids
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
types used as a `Result`'s error, and the actor loop-state types (the `Self`
of a `run(mut self)` whose body is an actor loop, and the type a loop's
`let mut` state is built from). The facts are names, not resolved paths.

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
| `item.fn.receiver.mut` | `&mut self` only on methods of an actor's loop state (proposal 0008) |
| `stmt.let.mut` | `let mut` only for a loop's state just before the loop, or for a value never reassigned and only lent as `&mut` (proposal 0008) |
| `pat.ident.mut` | a `mut` binding is never reassigned, only lent as `&mut` (proposal 0008) |
| `expr.assign` | assignment only to `self.field` in a `&mut self` method, or to the loop's state binding (proposal 0008) |
| `type.reference.mut` | no `&mut` of our own types other than `&mut self` (proposals 0008, 0023) |
| `item.impl.trait` | trait impls only for `Drop`, `Debug`, `Serialize`, `Deserialize` and `ProtocolHandler` (proposal 0009) |
| `generics.lifetime` | lifetime parameters only in a library trait impl (proposal 0009) |
| `type.lifetime` | lifetimes only in a library trait impl, where its signature demands them (proposal 0009) |
| `attr.derive` | `thiserror::Error` only on an enum or named struct that is an error type, named `…Error` or used as a `Result`'s error (proposal 0010) |
| `attr.error` | each error variant carries one `#[error("…")]` message, never `transparent` (proposal 0010) |
| `attr.from` | `#[from]` only on the one variant of an enum holding a given error type (proposal 0011) |
| `attr.serde` | `#[serde]` only with `transparent`, `tag`, `content`, `rename_all` and `rename` (proposal 0012) |
| `generics.type_param` | a type parameter only on a fn, with exactly one bound, `Serialize` or `DeserializeOwned`, and no `where` (proposal 0013) |
| `type.slice` | `&[T]` only as a parameter type or a `const` type (proposal 0014) |
| `type.array` | an array type only for bytes (`[u8; N]`) or a `const` table (proposal 0015) |
| `expr.array` | an array literal only passed straight to a call, or as a `const` table (proposal 0015) |
| `expr.repeat` | an array repeat only of a literal, as in `[0u8; N]` (proposal 0015) |
| `attr.cfg` | `cfg` only with `target_family = "wasm"`, `unix` or their `not` (proposal 0016) |
| `attr.cfg` | `cfg` only on a mod, use, fn or impl item, or as `#![cfg]` atop a test file (proposal 0016) |
| `attr.cfg` | a `cfg` fn, impl or use needs its counterpart for the other platform in the same file (proposal 0016) |
| `type.dyn` | `dyn` only as `Closure<dyn FnMut(..)>` (proposal 0017) |
| `expr.closure` | a closure outside a method call only as the argument of `Closure::new`, `unfold` or a Dioxus hook (proposals 0006, 0017, 0018) |
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
  checked as "never reassigned", 0011's foreign type and 0021's external API
  as "not ours", and a method's receiver is ours only when it is `self`. A
  library type that shares a name with one of ours counts as ours.
- **Some rules stay with the reviewer.** Syntax can't tell why a construct
  is there: 0002 `pub` only for another crate, 0006 `async fn` only for I/O,
  0012 serde only on wire types (checked by location), 0013 generics only at
  the wire boundary (checked by bound), 0014 a slice only for a read-only
  sequence, 0015 a byte array only where its size is part of a format, and
  0009 generics only where the trait demands them (any are allowed inside a
  listed trait impl). Neither can it see what a body does: 0017's callback
  only forwards to a channel, and 0019's `main` turns its `bool` into an
  enum. `docs/subset/compromises.md` lists these.
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
| `expr.if_let` | `if let` expression |
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
