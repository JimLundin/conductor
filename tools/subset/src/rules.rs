//! The shape rules of accepted proposals: where an allowed id may stand and
//! what form it takes. A rule applies only where its id is allowed; where the
//! id isn't allowed at all, the id itself is the violation. `subset rules`
//! prints this as the README table.

pub struct Rule {
    pub id: &'static str,
    pub message: &'static str,
}

const fn rule(id: &'static str, message: &'static str) -> Rule {
    Rule { id, message }
}

pub const VIS_CRATE: Rule = rule(
    "vis.restricted",
    "only `pub(crate)`, never `pub(super)` or `pub(in …)` (proposal 0003)",
);
pub const PUB_FIELD: Rule = rule(
    "vis.pub",
    "a field of a `pub(crate)` struct is `pub(crate)`, never `pub` (proposal 0003)",
);
pub const STRUCT_UPDATE: Rule = rule(
    "expr.struct.update",
    "the base of `..base` is a path, a constructor call or a method call (proposals 0004, 0036)",
);
pub const UNIT_TYPE: Rule = rule(
    "type.unit",
    "`()` as a type only in a type argument of a type we don't define, as in `Result<(), E>`, or as `fn main`'s return type (proposals 0005, 0023)",
);
pub const UNIT_VALUE: Rule = rule(
    "expr.unit",
    "`()` as a value only as an argument or field value of a fn, method or struct we don't define, as in `Ok(())` (proposals 0005, 0023)",
);
pub const ASYNC_BLOCK: Rule = rule(
    "expr.async",
    "an async block only as a spawn argument or the whole body of an `unfold`, `then` or `use_future` closure (proposal 0006)",
);
pub const TOKIO_MAIN: Rule = rule(
    "attr.tokio::main",
    "`#[tokio::main]` only on `fn main` (proposal 0006)",
);
pub const LOOP_POSITION: Rule = rule(
    "expr.loop",
    "loop must be the whole body of an actor run fn, after at most one `let mut` state (proposal 0007)",
);
pub const LOOP_TURN: Rule = rule(
    "expr.loop",
    "a loop turn takes one input (`select!`, a `match` of `select!`s, or one awaited call) and handles it with one call or `match` (proposal 0007)",
);
pub const BREAK: Rule = rule(
    "expr.break",
    "`break` only with a value, out of an actor loop (proposal 0007)",
);
pub const MUT_SELF: Rule = rule(
    "item.fn.receiver.mut",
    "`mut self` only on a fn whose body is an actor loop (proposal 0008)",
);
pub const REF_MUT_SELF: Rule = rule(
    "item.fn.receiver.mut",
    "`&mut self` only on methods of an actor's loop state, of a type of ours named in one of its field types or in a variant of an enum it owns (proposal 0008), or of a type in `endpoints` (proposal 0035)",
);
pub const LET_MUT: Rule = rule(
    "stmt.let.mut",
    "`let mut` only for a loop's state just before the loop, or for a value never reassigned and only lent as `&mut` (proposal 0008)",
);
pub const PAT_MUT: Rule = rule(
    "pat.ident.mut",
    "a `mut` binding is never reassigned, only lent as `&mut` (proposal 0008)",
);
const MUT_OURS: &str = "`mut` only on a value whose type isn't ours, on an actor's loop state or on a type in `endpoints`; to lend a library field of a value of ours as `&mut`, destructure the value and bind that field `mut` (proposals 0008, 0035)";
pub const LET_MUT_OURS: Rule = rule("stmt.let.mut", MUT_OURS);
pub const PARAM_MUT_OURS: Rule = rule("pat.ident.mut", MUT_OURS);
const FALLIBLE_OPERATOR: &str = "a trait in `fallible_operators` is never imported, named in a path or called as a method; write `checked_*` or `saturating_*` (proposal 0038)";
pub const OPERATOR_USE: Rule = rule("item.use", FALLIBLE_OPERATOR);
pub const OPERATOR_PATH: Rule = rule("expr.path", FALLIBLE_OPERATOR);
pub const OPERATOR_TYPE: Rule = rule("type.path", FALLIBLE_OPERATOR);
pub const OPERATOR_METHOD: Rule = rule("expr.method_call", FALLIBLE_OPERATOR);
const CONSUMING_READER: &str = "a reader in `consuming_readers` changes its buffer on every read, so a tuple of reads means something only by evaluation order; split the bytes with `split_at_checked` and match them, as in the bootstrap review's §A7 (proposal 0039)";
pub const READER_USE: Rule = rule("item.use", CONSUMING_READER);
pub const READER_PATH: Rule = rule("expr.path", CONSUMING_READER);
pub const READER_TYPE: Rule = rule("type.path", CONSUMING_READER);
pub const ASSIGN: Rule = rule(
    "expr.assign",
    "assignment only to `self.field` in a `&mut self` method, or to the loop's state binding (proposal 0008)",
);
pub const REF_MUT_OURS: Rule = rule(
    "type.reference.mut",
    "no `&mut` of our own types other than `&mut self` (proposals 0008, 0023)",
);
pub const DROP_DISCARD: Rule = rule(
    "expr.call",
    "`drop(..)` only of a named value that owns a resource released early; discard a value with `let _ = value;` (proposal 0025)",
);
pub const DROP_FN: Rule = rule(
    "expr.path",
    "`drop` passed as a function, as in `.map(drop)`, is a discard; write `let _ = value;` (proposal 0025)",
);
pub const OK_DISCARD: Rule = rule(
    "expr.method_call",
    "`.ok()` is not a discard; write `let _ = value;` (proposal 0025)",
);
const PREDICATE: &str = "a boolean expression (`matches!`, a comparison, `&&`, `||`, `!`) only as the body of a closure passed straight to a method in `predicate_adapters` (proposals 0028, 0037), or one comparison or `matches!` as a match guard (proposal 0040)";
pub const PREDICATE_MATCHES: Rule = rule("macro.matches", PREDICATE);
pub const PREDICATE_EQ: Rule = rule("expr.binary.eq", PREDICATE);
pub const PREDICATE_NE: Rule = rule("expr.binary.ne", PREDICATE);
pub const PREDICATE_LT: Rule = rule("expr.binary.lt", PREDICATE);
pub const PREDICATE_LE: Rule = rule("expr.binary.le", PREDICATE);
pub const PREDICATE_GT: Rule = rule("expr.binary.gt", PREDICATE);
pub const PREDICATE_GE: Rule = rule("expr.binary.ge", PREDICATE);
pub const PREDICATE_AND: Rule = rule("expr.binary.and", PREDICATE);
pub const PREDICATE_OR: Rule = rule("expr.binary.or", PREDICATE);
pub const PREDICATE_NOT: Rule = rule("expr.unary.not", PREDICATE);
pub const COUNT: Rule = rule(
    "expr.method_call",
    "count matching items with `filter(|x| matches!(x, ..)).count()`, not `filter_map(..).count()` (proposal 0028)",
);
pub const GUARD: Rule = rule(
    "expr.match.guard",
    "a match guard is one comparison (`==`, `!=`, `<`, `<=`, `>`, `>=`) or one `matches!`, with no `&&`, `||` or `!` (proposal 0040)",
);
pub const GUARD_BOUND: Rule = rule(
    "expr.match.guard",
    "a guard tests a value its arm's pattern bound; a guard on nothing the pattern bound is an `if` (proposal 0040)",
);
pub const GUARD_PATTERN: Rule = rule(
    "expr.match.guard",
    "a guard that compares with a literal, a variant or a constant, or a `matches!` on a place, is a pattern: write it in the arm (proposal 0040, bootstrap review §A3, §A4)",
);
pub const GUARD_FALLBACK: Rule = rule(
    "expr.match.guard",
    "a guarded arm needs a later arm with no guard whose pattern covers the guarded one, so the otherwise case is written out (proposal 0040)",
);
pub const MATCHES_FORM: Rule = rule(
    "macro.matches",
    "`matches!` takes a place or a call and a pattern with no `if` and no `true` / `false`, so no boolean rides through it (proposals 0028, 0040)",
);
pub const LIFT_FILTER: Rule = rule(
    "expr.method_call",
    "`Some(x).filter(..)` or `.find(..)` with one comparison is a match guard: `pat if a == b => ..` with an unguarded arm for the rest (proposal 0040)",
);
pub const LIFT_MATCHED: Rule = rule(
    "expr.method_call",
    "a `filter` of one comparison matched at once is a match guard: `Some(x) if a == b => ..` with an unguarded arm for the rest (proposal 0040)",
);
pub const LIFT_EQ: Rule = rule(
    "expr.method_call",
    "a comparison by name (`a.eq(&b)`, `a.lt(&b)`, `a.cmp(&b).is_eq()`, `PartialEq::eq`) is a `bool` outside a predicate; write a match guard `pat if a == b => ..` (proposal 0040)",
);
pub const FOR_BODY: Rule = rule(
    "expr.for",
    "a `for` body holds only calls, as `f(item);`, `f(item)?;`, `x.m(item).await?;` or `let _ = f(item);`, each building nothing; build or select values with the adapters (`map`, `filter`, `fold`, `try_fold`, ..) (proposal 0041)",
);
pub const FOR_ITER: Rule = rule(
    "expr.for",
    "a `for` iterates a list built before it or by adapters, with no block, assignment or branch in its iterator expression (proposal 0041)",
);
pub const FOR_EACH: Rule = rule(
    "expr.method_call",
    "a loop whose body performs side effects is `for item in list { .. }`, not `for_each` or `try_for_each` (proposal 0041)",
);
pub const STREAM_EFFECTS: Rule = rule(
    "expr.method_call",
    "awaited effects run in order as `for item in list { f(item).await?; }`, not `stream::iter(..).then(..).try_collect()` into `()` (proposal 0041)",
);

pub const IMPL_TRAIT: Rule = rule(
    "item.impl.trait",
    "trait impls only for `Drop`, `Debug`, `Serialize`, `Deserialize` and `ProtocolHandler` (proposal 0009)",
);
pub const LIFETIME_PARAM: Rule = rule(
    "generics.lifetime",
    "lifetime parameters only in a library trait impl (proposal 0009)",
);
pub const LIFETIME: Rule = rule(
    "type.lifetime",
    "lifetimes only in a library trait impl, where its signature demands them (proposal 0009)",
);
pub const STATIC_STR: Rule = rule(
    "type.lifetime.static_str",
    "`&'static str` only as a field type of a struct that is the row type of a `const` table and is built nowhere else (proposal 0042)",
);
pub const KEYED_TABLE: Rule = rule(
    "expr.method_call",
    "a `const` table searched for the row whose string field equals a value is a keyed lookup; write a `match` on the value, as in `match os { \"macos\" => .., _ => .. }` (proposal 0042)",
);
const RANGE_BY_NAME: &str = "a range built by name (`RangeInclusive::new`, `Range { .. }`, `RangeFull`, a pair of `Bound`s) is a range expression (`expr.range`) under another name; it stands only as the iterator of a `for`, the receiver of an iterator method or the argument of `zip` or `chain`, never as an index or a slice argument: split bytes with `split_at_checked` (bootstrap review §A7, rtc-ports review)";
pub const RANGE_CALL: Rule = rule("expr.call", RANGE_BY_NAME);
pub const RANGE_STRUCT: Rule = rule("expr.struct", RANGE_BY_NAME);
pub const RANGE_PATH: Rule = rule("expr.path", RANGE_BY_NAME);
pub const RANGE_BOUNDS: Rule = rule("expr.tuple", RANGE_BY_NAME);
pub const TUPLE_TABLE: Rule = rule(
    "expr.tuple",
    "a table whose rows have more than two fields is a list of a named struct, not of tuples (proposal 0001, owner ruling 2026-11-12)",
);
pub const THISERROR_HOST: Rule = rule(
    "attr.derive",
    "`thiserror::Error` only on an enum or named struct that is an error type, named `…Error` or used as a `Result`'s error (proposal 0010)",
);
pub const ORD_HOST: Rule = rule(
    "attr.derive",
    "`PartialOrd` and `Ord` only on a newtype struct whose order means something (proposal 0024)",
);
pub const ORD_INNER: Rule = rule(
    "expr.method_call",
    "compare an ordered newtype as a whole, `a.cmp(&b)`, never through `.0` (proposal 0024)",
);
pub const ERROR_MESSAGE: Rule = rule(
    "attr.error",
    "each error variant carries one `#[error(\"…\")]` message, never `transparent` (proposal 0010)",
);
pub const FROM_ONCE: Rule = rule(
    "attr.from",
    "`#[from]` only on the one variant of an enum holding a given error type (proposal 0011)",
);
pub const SERDE_KEYS: Rule = rule(
    "attr.serde",
    "`#[serde]` only with `transparent`, `tag`, `content`, `rename_all` and `rename` (proposal 0012)",
);
pub const TYPE_PARAM: Rule = rule(
    "generics.type_param",
    "a type parameter only on a fn, with exactly one bound, `Serialize` or `DeserializeOwned`, and no `where` (proposal 0013), or as the unbounded `T` of an `ask` helper (proposal 0026)",
);
pub const IMPL_TRAIT_ARG: Rule = rule(
    "type.impl_trait.arg",
    "`impl Trait` as an argument only as `build: impl FnOnce(oneshot::Sender<T>) -> Msg`, the last parameter of a private `ask<T>` helper (proposal 0026)",
);
pub const SLICE: Rule = rule(
    "type.slice",
    "`&[T]` only as a parameter type or a `const` type; a `const` list is `const NAME: &[T] = &[..]` (proposal 0014)",
);
pub const ARRAY_TYPE: Rule = rule(
    "type.array",
    "an array type only for bytes whose size is part of a format or API (`[u8; N]`); a `const` list is `&[T]` (proposals 0014, 0015)",
);
pub const ARRAY_VALUE: Rule = rule(
    "expr.array",
    "an array literal only passed straight to a call, as in `f([a, b])` or `f(&[a, b])`, or as a `const` value (proposal 0015)",
);
pub const REPEAT: Rule = rule(
    "expr.repeat",
    "an array repeat only of a literal, as in `[0u8; N]` (proposal 0015)",
);
pub const CFG_PREDICATE: Rule = rule(
    "attr.cfg",
    "`cfg` only with `target_family = \"wasm\"`, `unix` or their `not` (proposal 0016)",
);
pub const CFG_HOST: Rule = rule(
    "attr.cfg",
    "`cfg` only on a mod, use, fn or impl item, or as `#![cfg]` atop a test file (proposal 0016)",
);
pub const CFG_PAIR: Rule = rule(
    "attr.cfg",
    "a `cfg` fn, impl or use needs its counterpart for the other platform in the same file (proposal 0016)",
);
pub const DYN: Rule = rule(
    "type.dyn",
    "`dyn` only as `Closure<dyn FnMut(..)>` (proposal 0017)",
);
pub const CLOSURE_METHOD: Rule = rule(
    "expr.closure.adapter",
    "a closure passed to a method only to a method in `closure_methods` or `predicate_adapters`: the iterator, `Option` and `Result` methods and the library methods the config lists (proposals 0006, 0026, 0028, 0037)",
);
pub const CLOSURE: Rule = rule(
    "expr.closure",
    "a closure outside a method call only as the argument of `Closure::new`, `unfold`, `ask` or a Dioxus hook (proposals 0006, 0017, 0018, 0026)",
);
pub const GLOB: Rule = rule(
    "item.use.glob",
    "a glob import only for `dioxus::prelude::*` (proposal 0018)",
);
pub const ALLOW: Rule = rule(
    "attr.allow",
    "only `#![allow(clippy::useless_format)]` at the crate root (proposal 0018)",
);
pub const BOOL_TYPE: Rule = rule(
    "type.bool",
    "`bool` only in a type argument of a type we don't define, or as a flag field of a clap `Parser` struct (proposals 0019, 0023)",
);
pub const VALUE_PARSER: Rule = rule(
    "attr.arg",
    "a choice between named values is a `ValueEnum`, not `value_parser = [...]` (proposal 0019)",
);
pub const TOKIO_TEST: Rule = rule(
    "attr.tokio::test",
    "`#[tokio::test]` takes no arguments and marks an `async fn` (proposal 0020)",
);
pub const BOOL_LIT: Rule = rule(
    "expr.lit.bool",
    "`true` / `false` only as an argument or field value of a fn, method or struct we don't define (proposal 0023)",
);
pub const ENV: Rule = rule(
    "macro.env",
    "`env!` only for `CARGO_PKG_VERSION`, `CARGO_PKG_NAME` and `CARGO_MANIFEST_DIR` (proposal 0022)",
);

pub const RULES: &[&Rule] = &[
    &VIS_CRATE,
    &PUB_FIELD,
    &STRUCT_UPDATE,
    &UNIT_TYPE,
    &UNIT_VALUE,
    &ASYNC_BLOCK,
    &TOKIO_MAIN,
    &LOOP_POSITION,
    &LOOP_TURN,
    &BREAK,
    &MUT_SELF,
    &REF_MUT_SELF,
    &LET_MUT,
    &PAT_MUT,
    &LET_MUT_OURS,
    &PARAM_MUT_OURS,
    &ASSIGN,
    &REF_MUT_OURS,
    &DROP_DISCARD,
    &DROP_FN,
    &OK_DISCARD,
    &OPERATOR_USE,
    &OPERATOR_PATH,
    &OPERATOR_TYPE,
    &OPERATOR_METHOD,
    &READER_USE,
    &READER_PATH,
    &READER_TYPE,
    &PREDICATE_MATCHES,
    &PREDICATE_EQ,
    &PREDICATE_NE,
    &PREDICATE_LT,
    &PREDICATE_LE,
    &PREDICATE_GT,
    &PREDICATE_GE,
    &PREDICATE_AND,
    &PREDICATE_OR,
    &PREDICATE_NOT,
    &COUNT,
    &GUARD,
    &GUARD_BOUND,
    &GUARD_PATTERN,
    &GUARD_FALLBACK,
    &MATCHES_FORM,
    &LIFT_FILTER,
    &LIFT_MATCHED,
    &LIFT_EQ,
    &FOR_BODY,
    &FOR_ITER,
    &FOR_EACH,
    &STREAM_EFFECTS,
    &IMPL_TRAIT,
    &LIFETIME_PARAM,
    &LIFETIME,
    &STATIC_STR,
    &KEYED_TABLE,
    &RANGE_CALL,
    &RANGE_STRUCT,
    &RANGE_PATH,
    &RANGE_BOUNDS,
    &TUPLE_TABLE,
    &THISERROR_HOST,
    &ORD_HOST,
    &ORD_INNER,
    &ERROR_MESSAGE,
    &FROM_ONCE,
    &SERDE_KEYS,
    &TYPE_PARAM,
    &IMPL_TRAIT_ARG,
    &SLICE,
    &ARRAY_TYPE,
    &ARRAY_VALUE,
    &REPEAT,
    &CFG_PREDICATE,
    &CFG_HOST,
    &CFG_PAIR,
    &DYN,
    &CLOSURE_METHOD,
    &CLOSURE,
    &GLOB,
    &ALLOW,
    &BOOL_TYPE,
    &VALUE_PARSER,
    &TOKIO_TEST,
    &BOOL_LIT,
    &ENV,
];

const COMPOUND_ASSIGNMENT: &str =
    "compound assignment is not allowed; write `x = x.checked_add(1)?` (proposal 0008)";
const INTERIOR_MUTABILITY: &str = "interior mutability stays out of the subset (proposal 0008)";

/// The reason for an id that a proposal names as never allowed, in place of
/// the id's plain meaning.
pub fn forbidden(id: &str) -> Option<&'static str> {
    match id {
        _ if id.starts_with("expr.binary.") && id.ends_with("_assign") => Some(COMPOUND_ASSIGNMENT),
        _ if id.starts_with("type.interior_mut.") || id.starts_with("expr.interior_mut.") => {
            Some(INTERIOR_MUTABILITY)
        }
        _ => None,
    }
}

pub fn markdown_table() -> String {
    let rows = RULES
        .iter()
        .map(|r| format!("| `{}` | {} |\n", r.id, r.message.replace('|', "\\|")));
    std::iter::once("| Id | Rule |\n|---|---|\n".to_string())
        .chain(rows)
        .collect()
}
