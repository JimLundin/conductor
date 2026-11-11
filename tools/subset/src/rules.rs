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
    "the base of `..base` is a path or a constructor call (proposal 0004)",
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
const PREDICATE: &str = "a boolean expression (`matches!`, a comparison, `&&`, `||`, `!`) only as the body of a closure passed straight to `filter`, `find`, `any`, `all`, `position`, `take_while`, `skip_while` or `retain` (proposal 0028)";
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
    &ASSIGN,
    &REF_MUT_OURS,
    &DROP_DISCARD,
    &DROP_FN,
    &OK_DISCARD,
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
    &IMPL_TRAIT,
    &LIFETIME_PARAM,
    &LIFETIME,
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
