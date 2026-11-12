//! Checks the shape rules in `rules`: where an id stands and what form it
//! takes. Each finding names the rule it breaks. `main` reports a finding only
//! where the rule's id is allowed, since elsewhere the id itself is reported.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ops::Range;

use proc_macro2::{Span, TokenTree};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{
    Attribute, Expr, Fields, FnArg, GenericArgument, GenericParam, ImplItem, Item, Lit, Meta,
    PathArguments, ReturnType, Type, TypeParamBound, UseTree, Visibility,
};

use crate::actor;
use crate::facts::{self, Facts};
use crate::imports::{FileNames, use_paths};
use crate::named::{Named, Operator};
use crate::rules::{self, Rule};
use crate::table::{self, Tables};

pub struct Finding {
    pub rule: &'static Rule,
    pub line: usize,
    pub col: usize,
    /// The byte offset of the token the finding points at.
    pub at: usize,
}

/// A node whose shape an item we don't define fixes: the score charges its
/// tokens the boundary weight.
pub struct Mark {
    /// The node's bytes, equal to the extent of its occurrence.
    pub extent: Range<usize>,
    pub when: Fixed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fixed {
    /// The node exists only because the library demands it: a trait impl,
    /// a library attribute or macro, a binding lent to a library as `&mut`.
    Always,
    /// The node is an argument, field value or type argument of an item we
    /// don't define. Any construct can stand there; the score relieves only
    /// the restricted ones.
    IfRestricted,
}

pub struct Shape {
    pub findings: Vec<Finding>,
    pub boundary: Vec<Mark>,
}

/// What a file is, as far as rules that depend on the file care.
pub struct FileKind {
    pub test: bool,
    pub crate_root: bool,
}

/// The name lists in the config that shape rules read.
pub struct Lists<'a> {
    /// Types of ours whose methods may take `&mut self` anywhere (proposal
    /// 0035).
    pub endpoints: &'a [String],
    /// Methods whose closure argument may have a boolean expression as its
    /// body (proposals 0028, 0037).
    pub predicate_adapters: &'a [String],
    /// Methods that may take a closure argument, besides the predicate
    /// adapters.
    pub closure_methods: &'a [String],
    /// The std `Iterator` methods (`[weights] adapters`).
    pub adapters: &'a [String],
    /// Operator traits never named or called by name (proposal 0038).
    pub fallible_operators: &'a [Operator],
    /// Types and traits never named (proposal 0039).
    pub consuming_readers: &'a [Named],
}

impl Lists<'_> {
    fn predicate(&self, method: &str) -> bool {
        self.predicate_adapters.iter().any(|m| m == method)
    }

    fn takes_closure(&self, method: &str) -> bool {
        self.predicate(method) || self.closure_methods.iter().any(|m| m == method)
    }

    /// Whether a method called on a range iterates it.
    fn iterates(&self, method: &str) -> bool {
        self.takes_closure(method) || self.adapters.iter().any(|m| m == method)
    }
}

/// The filtering adapters of proposal 0028: the default `predicate_adapters`.
pub const PREDICATE_ADAPTERS: &[&str] = &[
    "filter",
    "find",
    "any",
    "all",
    "position",
    "take_while",
    "skip_while",
    "retain",
];
/// The standard library's `Iterator`, `Option` and `Result` methods that
/// take a closure: the default `closure_methods`.
pub const CLOSURE_METHODS: &[&str] = &[
    "and_then",
    "filter_map",
    "find_map",
    "flat_map",
    "fold",
    "for_each",
    "get_or_insert_with",
    "inspect",
    "inspect_err",
    "is_err_and",
    "is_none_or",
    "is_ok_and",
    "is_some_and",
    "map",
    "map_err",
    "map_or",
    "map_or_else",
    "map_while",
    "max_by",
    "max_by_key",
    "min_by",
    "min_by_key",
    "ok_or_else",
    "or_else",
    "partition",
    "reduce",
    "rfind",
    "rposition",
    "scan",
    "try_fold",
    "try_for_each",
    "unwrap_or_else",
];

pub fn default_predicate_adapters() -> Vec<String> {
    PREDICATE_ADAPTERS.iter().map(ToString::to_string).collect()
}

pub fn default_closure_methods() -> Vec<String> {
    CLOSURE_METHODS.iter().map(ToString::to_string).collect()
}

pub fn check(file: &syn::File, kind: &FileKind, facts: &Facts, lists: &Lists) -> Shape {
    let names = FileNames::gather(file, facts);
    let operator_methods = imported_operator_methods(&names, lists.fallible_operators);
    let mut checker = Checker {
        facts,
        kind,
        lists,
        exprs: HashMap::new(),
        types: HashMap::new(),
        cfg_hosts: HashMap::new(),
        cfg_items: CfgItems::gather(file),
        tables: Tables::gather(file),
        static_strs: HashSet::new(),
        range_sources: HashSet::new(),
        names,
        operator_methods,
        root_allows: Vec::new(),
        impls: Vec::new(),
        fns: Vec::new(),
        loops: Vec::new(),
        sig_generics: None,
        ask_generics: None,
        in_return: false,
        in_let: false,
        predicates: HashSet::new(),
        predicate_macros: HashSet::new(),
        in_predicate: 0,
        out: Vec::new(),
        boundary: Vec::new(),
    };
    checker.visit_file(file);
    Shape {
        findings: checker.out,
        boundary: checker.boundary,
    }
}

/// The syntactic slot a node fills in its parent. Nodes not registered by a
/// parent are in `Other`.
#[derive(Clone, PartialEq)]
enum Slot {
    Other,
    CallArg(Callee),
    MethodArg {
        method: String,
        on_self: bool,
    },
    FieldValue {
        ours: bool,
    },
    ClosureBody(Box<Slot>),
    ConstInit,
    ConstType,
    Param,
    RefTarget(Box<Slot>),
    GenericArg {
        segment: String,
        ours: bool,
    },
    MainReturn,
    ClapField,
    /// A field type of a struct that is a `const` table's row (proposal
    /// 0042).
    RowField,
    /// The `build` parameter of an `ask` helper (proposal 0026).
    AskBuild,
}

#[derive(Clone, PartialEq)]
struct Callee {
    path: Vec<String>,
    ours: bool,
}

impl Callee {
    fn last(&self) -> &str {
        self.path.last().map_or("", String::as_str)
    }

    fn ends_with(&self, tail: &[&str]) -> bool {
        self.path.len() >= tail.len()
            && self.path[self.path.len() - tail.len()..]
                .iter()
                .zip(tail)
                .all(|(a, b)| a == b)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum ImplKind {
    Inherent,
    /// A trait on the 0009 list, whose signatures may demand generics.
    Library,
    OtherTrait,
}

struct ImplCtx {
    self_ty: Option<String>,
    kind: ImplKind,
    /// The trait is not one we define.
    foreign: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Receiver {
    None,
    Shared,
    Owned,
    OwnedMut,
    RefMut,
}

struct FnCtx {
    receiver: Receiver,
    actor_loop: Option<*const syn::ExprLoop>,
    state: Option<*const syn::Local>,
    state_names: Vec<String>,
    /// Plain names assigned with `=` anywhere in the body.
    assigned: BTreeSet<String>,
    /// Each parameter bound to a plain name, with its type's last segment.
    params: HashMap<String, String>,
    /// Names bound by `let name = Some(..)`, an `Option` built in place.
    some_bindings: BTreeSet<String>,
    /// Names bound to a stream that starts at `stream::iter`.
    stream_bindings: BTreeSet<String>,
    /// Names bound to a `const` table or a chain over one (proposal 0042).
    table_bindings: HashMap<String, Expr>,
}

/// The rules of proposals 0038 and 0039 at one kind of position.
struct NamedRules {
    operator: &'static Rule,
    reader: &'static Rule,
}

const USE_RULES: NamedRules = NamedRules {
    operator: &rules::OPERATOR_USE,
    reader: &rules::READER_USE,
};
const PATH_RULES: NamedRules = NamedRules {
    operator: &rules::OPERATOR_PATH,
    reader: &rules::READER_PATH,
};
const TYPE_RULES: NamedRules = NamedRules {
    operator: &rules::OPERATOR_TYPE,
    reader: &rules::READER_TYPE,
};

const LIBRARY_TRAITS: &[&str] = &[
    "Drop",
    "Debug",
    "Serialize",
    "Deserialize",
    "ProtocolHandler",
];
const CODEC_BOUNDS: &[&str] = &["Serialize", "DeserializeOwned"];
const CLOSURE_CALLEES: &[&str] = &["unfold", "use_signal", "use_future", "use_hook", "ask"];
/// Macros of the standard library. Every other macro is a library's.
const STD_MACROS: &[&str] = &[
    "assert",
    "assert_eq",
    "assert_ne",
    "cfg",
    "column",
    "compile_error",
    "concat",
    "dbg",
    "debug_assert",
    "debug_assert_eq",
    "debug_assert_ne",
    "env",
    "eprint",
    "eprintln",
    "file",
    "format",
    "format_args",
    "include_bytes",
    "include_str",
    "line",
    "matches",
    "panic",
    "print",
    "println",
    "stringify",
    "todo",
    "unimplemented",
    "unreachable",
    "vec",
    "write",
    "writeln",
];
/// Attributes of the language. Every other attribute is a library's.
const BUILTIN_ATTRS: &[&str] = &[
    "allow",
    "automatically_derived",
    "cfg",
    "cfg_attr",
    "cold",
    "deny",
    "deprecated",
    "derive",
    "doc",
    "expect",
    "forbid",
    "ignore",
    "inline",
    "macro_export",
    "must_use",
    "non_exhaustive",
    "path",
    "repr",
    "should_panic",
    "test",
    "track_caller",
    "warn",
];
const SERDE_KEYS: &[&str] = &["transparent", "tag", "content", "rename_all", "rename"];
const CFG_PAIRS: &[(&str, &str)] = &[
    ("target_family=\"wasm\"", "not(target_family=\"wasm\")"),
    ("unix", "not(unix)"),
];

struct Checker<'a> {
    facts: &'a Facts,
    kind: &'a FileKind,
    lists: &'a Lists<'a>,
    exprs: HashMap<*const Expr, Slot>,
    types: HashMap<*const Type, Slot>,
    /// `cfg` attributes on a host the rule allows, with the item they need a
    /// counterpart for.
    cfg_hosts: HashMap<*const Attribute, Option<CfgItem>>,
    cfg_items: CfgItems,
    tables: Tables,
    /// The `'static` of each `&'static str`, which proposal 0042's rule
    /// checks in place of 0009's.
    static_strs: HashSet<*const syn::Lifetime>,
    /// Expressions that a `for`, an iterator method or `zip` / `chain`
    /// iterate, where a range built by name may stand.
    range_sources: HashSet<*const Expr>,
    names: FileNames,
    /// The methods of the fallible operator traits this file imports.
    operator_methods: BTreeSet<String>,
    root_allows: Vec<*const Attribute>,
    impls: Vec<ImplCtx>,
    fns: Vec<FnCtx>,
    /// For each enclosing loop, whether it is an actor loop.
    loops: Vec<bool>,
    sig_generics: Option<*const syn::Generics>,
    /// The generics of an `ask` helper in the shape of proposal 0026.
    ask_generics: Option<*const syn::Generics>,
    in_return: bool,
    in_let: bool,
    /// The boolean expressions and `matches!` calls that make up a
    /// predicate closure's body (proposal 0028).
    predicates: HashSet<*const Expr>,
    predicate_macros: HashSet<*const syn::Macro>,
    /// How many predicate closure bodies enclose the node being visited.
    in_predicate: usize,
    out: Vec<Finding>,
    boundary: Vec<Mark>,
}

impl Checker<'_> {
    fn flag(&mut self, span: Span, rule: &'static Rule) {
        let start = span.start();
        self.out.push(Finding {
            rule,
            line: start.line,
            col: start.column + 1,
            at: span.byte_range().start,
        });
    }

    fn mark(&mut self, node: Span, when: Fixed) {
        self.boundary.push(Mark {
            extent: node.byte_range(),
            when,
        });
    }

    fn in_foreign_impl(&self) -> bool {
        self.impls.last().is_some_and(|i| i.foreign)
    }

    fn expr_slot(&self, expr: &Expr) -> Slot {
        self.exprs
            .get(&std::ptr::from_ref(expr))
            .cloned()
            .unwrap_or(Slot::Other)
    }

    fn type_slot(&self, ty: &Type) -> Slot {
        self.types
            .get(&std::ptr::from_ref(ty))
            .cloned()
            .unwrap_or(Slot::Other)
    }

    fn set_expr(&mut self, expr: &Expr, slot: Slot) {
        self.exprs.insert(std::ptr::from_ref(expr), slot);
    }

    fn set_type(&mut self, ty: &Type, slot: Slot) {
        self.types.insert(std::ptr::from_ref(ty), slot);
    }

    /// A `const` table of tuples with a `&str` in them, whose row is a
    /// named struct under proposal 0042.
    fn check_tuple_rows(&mut self, ty: &Type) {
        if let Some(row @ Type::Tuple(_)) = table::element(ty)
            && table::holds_str(row)
        {
            self.flag(row.span(), &rules::STR_TUPLE_ROW);
        }
    }

    fn in_library_impl(&self) -> bool {
        self.impls
            .last()
            .is_some_and(|i| i.kind == ImplKind::Library)
    }

    fn mark_ask(&mut self, vis: &Visibility, sig: &syn::Signature) {
        if let Some(build) = ask_build(vis, sig) {
            self.ask_generics = Some(std::ptr::from_ref(&sig.generics));
            self.set_type(build, Slot::AskBuild);
        }
    }

    fn enter_fn(&mut self, sig: &syn::Signature, block: &syn::Block) {
        let receiver = match sig.inputs.first() {
            Some(FnArg::Receiver(r)) => match (r.reference.is_some(), r.mutability.is_some()) {
                (true, true) => Receiver::RefMut,
                (true, false) => Receiver::Shared,
                (false, true) => Receiver::OwnedMut,
                (false, false) => Receiver::Owned,
            },
            _ => Receiver::None,
        };
        let actor = actor::actor_loop(block);
        let state = actor.as_ref().and_then(|a| a.state);
        self.fns.push(FnCtx {
            receiver,
            actor_loop: actor.as_ref().map(|a| std::ptr::from_ref(a.body)),
            state: state.map(std::ptr::from_ref),
            state_names: state
                .map(|s| actor::mut_bindings(&s.pat))
                .unwrap_or_default(),
            assigned: assigned_names(block),
            params: param_types(sig),
            some_bindings: bound_to(block, |init, _| some_call(init)),
            stream_bindings: bound_to(block, stream_root),
            table_bindings: self.tables.bindings(block),
        });
        if let Some(FnArg::Receiver(r)) = sig.inputs.first() {
            self.check_receiver(r);
        }
        self.check_mut_params(sig);
    }

    fn check_receiver(&mut self, r: &syn::Receiver) {
        let Some(token) = r.mutability else {
            return;
        };
        if self.in_foreign_impl() {
            self.mark(r.span(), Fixed::Always);
        }
        let ctx = self
            .fns
            .last()
            .map(|f| (f.receiver, f.actor_loop.is_some()));
        match ctx {
            Some((Receiver::OwnedMut, false)) => self.flag(token.span, &rules::MUT_SELF),
            Some((Receiver::RefMut, _)) => {
                let state = self.impls.last().is_some_and(|i| {
                    i.kind == ImplKind::Library
                        || i.self_ty.as_ref().is_some_and(|t| {
                            self.facts.loop_states.contains(t)
                                || self.facts.owned_by_state.contains(t)
                                || self.lists.endpoints.contains(t)
                        })
                });
                if !state {
                    self.flag(token.span, &rules::REF_MUT_SELF);
                }
            }
            _ => {}
        }
    }

    /// The type of ours that `segments` name, as in `Attempt`, `Self` or
    /// `crate::link::Attempt`.
    fn our_type(&self, segments: &[String]) -> Option<String> {
        let (last, before) = segments.split_last()?;
        let name = match (last.as_str(), before.first()) {
            ("Self", None) => return self.impls.last()?.self_ty.clone(),
            (_, None) => {
                return self
                    .names
                    .bare_type_is_ours(last, self.facts)
                    .then(|| last.clone());
            }
            (_, Some(first)) => self
                .names
                .rooted_in_ours(first, self.facts)
                .then(|| last.clone())?,
        };
        self.facts.types.contains(&name).then_some(name)
    }

    /// The type of ours a `mut` binding holds, where the syntax names it:
    /// its declared type, the struct or variant its value builds, or the
    /// type whose associated fn its value comes from. A type that 0008 or
    /// 0035 lets a binding hold `mut` doesn't count.
    fn mut_of_ours(&self, ty: Option<&Type>, init: Option<&Expr>) -> Option<String> {
        let ours = match (ty, init.map(actor::strip)) {
            (Some(Type::Path(t)), _) if t.qself.is_none() => self.our_type(&segments(&t.path)),
            (Some(_), _) => None,
            (None, Some(Expr::Struct(e))) => {
                let names = segments(&e.path);
                self.our_type(&names)
                    .or_else(|| self.our_type(names.split_last()?.1))
            }
            (None, Some(Expr::Call(call))) => match &*call.func {
                Expr::Path(p) => self.our_type(segments(&p.path).split_last()?.1),
                _ => None,
            },
            (None, _) => None,
        }?;
        let exempt = self.facts.loop_states.contains(&ours) || self.lists.endpoints.contains(&ours);
        (!exempt).then_some(ours)
    }

    fn check_mut_params(&mut self, sig: &syn::Signature) {
        for arg in &sig.inputs {
            if let FnArg::Typed(t) = arg
                && let syn::Pat::Ident(p) = &*t.pat
                && p.mutability.is_some()
                && self.mut_of_ours(Some(&t.ty), None).is_some()
            {
                self.flag(p.span(), &rules::PARAM_MUT_OURS);
            }
        }
    }

    /// The rule a path breaks when it names a fallible operator trait or a
    /// consuming reader, as the code writes it. A bare name the file
    /// defines or imports from a path of ours is ours.
    fn named_rule(&self, segments: &[String], rules: &NamedRules) -> Option<&'static Rule> {
        let at = |item: &Named| {
            item.found_in(segments).filter(|&i| {
                i > 0
                    || !(self.names.ours.contains(item.name())
                        || self.names.defined.contains(item.name()))
            })
        };
        let operator = self
            .lists
            .fallible_operators
            .iter()
            .any(|o| at(&o.item).is_some());
        let reader = self.lists.consuming_readers.iter().any(|r| at(r).is_some());
        match (operator, reader) {
            (true, _) => Some(rules.operator),
            (false, true) => Some(rules.reader),
            (false, false) => None,
        }
    }

    fn assigned_here(&self, names: &[String]) -> bool {
        self.fns
            .last()
            .is_some_and(|f| names.iter().any(|n| f.assigned.contains(n)))
    }

    fn check_fn_attrs(&mut self, attrs: &[Attribute], sig: &syn::Signature) {
        for attr in attrs {
            match path_string(attr.path()).as_str() {
                "tokio::test" => {
                    let bare = matches!(attr.meta, Meta::Path(_));
                    if !bare || sig.asyncness.is_none() {
                        self.flag(attr.pound_token.span, &rules::TOKIO_TEST);
                    }
                }
                "tokio::main" if sig.ident != "main" => {
                    self.flag(attr.pound_token.span, &rules::TOKIO_MAIN);
                }
                _ => {}
            }
        }
    }

    fn mark_cfg_hosts(&mut self, attrs: &[Attribute], item: Option<CfgItem>) {
        for attr in attrs.iter().filter(|a| is_cfg(a)) {
            self.cfg_hosts
                .insert(std::ptr::from_ref(attr), item.clone());
        }
    }

    fn check_cfg(&mut self, attr: &Attribute) {
        let Some(predicate) = cfg_predicate(attr) else {
            return self.flag(attr.pound_token.span, &rules::CFG_PREDICATE);
        };
        let Some(host) = self.cfg_hosts.get(&std::ptr::from_ref(attr)).cloned() else {
            return self.flag(attr.pound_token.span, &rules::CFG_HOST);
        };
        let Some(item) = host else {
            return;
        };
        let wanted = CfgItem {
            predicate: complement(&predicate),
            ..item
        };
        if !self.kind.test && !self.cfg_items.0.contains(&wanted) {
            self.flag(attr.pound_token.span, &rules::CFG_PAIR);
        }
    }

    fn check_derived(&mut self, attrs: &[Attribute], ident: &syn::Ident, data: Derived<'_>) {
        let Some(derive) = attrs.iter().find(|a| derives(a, "thiserror::Error")) else {
            return;
        };
        let name = ident.to_string();
        let named_error = name.ends_with("Error") || self.facts.error_types.contains(&name);
        let host = !matches!(data, Derived::Struct(Fields::Unnamed(_) | Fields::Unit));
        if !(named_error && host) {
            self.flag(derive.pound_token.span, &rules::THISERROR_HOST);
        }
        match data {
            Derived::Struct(_) => self.check_message(attrs, ident.span()),
            Derived::Enum(variants) => {
                for v in variants {
                    self.check_message(&v.attrs, v.ident.span());
                }
            }
        }
    }

    fn check_ord(&mut self, attrs: &[Attribute], newtype: bool) {
        let ord = attrs
            .iter()
            .find(|a| facts::derives(a, "Ord") || facts::derives(a, "PartialOrd"));
        if let (Some(derive), false) = (ord, newtype) {
            self.flag(derive.pound_token.span, &rules::ORD_HOST);
        }
    }

    /// Whether `x.0` reads the field of an ordered newtype: `x` is `self` in
    /// an impl of one, or a parameter declared as one.
    fn ordered_newtype_field(&self, expr: &Expr) -> bool {
        let Expr::Field(field) = expr else {
            return false;
        };
        let (syn::Member::Unnamed(index), Expr::Path(base)) = (&field.member, &*field.base) else {
            return false;
        };
        let Some(name) = base.path.get_ident().map(ToString::to_string) else {
            return false;
        };
        let ty = match name.as_str() {
            "self" => self.impls.last().and_then(|i| i.self_ty.clone()),
            _ => self.fns.last().and_then(|f| f.params.get(&name).cloned()),
        };
        index.index == 0 && ty.is_some_and(|t| self.facts.ord_newtypes.contains(&t))
    }

    /// Marks the boolean tree of a predicate closure's body: `&&`, `||` and
    /// `!` over comparisons, `matches!` and other leaves.
    fn mark_predicate(&mut self, expr: &Expr) {
        match expr {
            Expr::Paren(e) => self.mark_predicate(&e.expr),
            Expr::Block(e) if e.label.is_none() => {
                if let [syn::Stmt::Expr(tail, None)] = e.block.stmts.as_slice() {
                    self.mark_predicate(tail);
                }
            }
            Expr::Binary(e) => match e.op {
                syn::BinOp::And(_) | syn::BinOp::Or(_) => {
                    self.predicates.insert(std::ptr::from_ref(expr));
                    self.mark_predicate(&e.left);
                    self.mark_predicate(&e.right);
                }
                syn::BinOp::Eq(_)
                | syn::BinOp::Ne(_)
                | syn::BinOp::Lt(_)
                | syn::BinOp::Le(_)
                | syn::BinOp::Gt(_)
                | syn::BinOp::Ge(_) => {
                    self.predicates.insert(std::ptr::from_ref(expr));
                }
                _ => {}
            },
            Expr::Unary(e) if matches!(e.op, syn::UnOp::Not(_)) => {
                self.predicates.insert(std::ptr::from_ref(expr));
                self.mark_predicate(&e.expr);
            }
            Expr::Macro(e) if e.mac.path.is_ident("matches") => {
                self.predicate_macros.insert(std::ptr::from_ref(&e.mac));
            }
            _ => {}
        }
    }

    fn check_message(&mut self, attrs: &[Attribute], at: Span) {
        let messages: Vec<&Attribute> = attrs
            .iter()
            .filter(|a| a.path().is_ident("error"))
            .collect();
        match messages.as_slice() {
            [one] if has_message(one) => {}
            [] => self.flag(at, &rules::ERROR_MESSAGE),
            [first, ..] => self.flag(first.pound_token.span, &rules::ERROR_MESSAGE),
        }
    }

    fn check_from(&mut self, variants: &Punctuated<syn::Variant, syn::Token![,]>) {
        let held: Vec<&Type> = variants
            .iter()
            .flat_map(|v| v.fields.iter().map(|f| &f.ty))
            .collect();
        for field in variants.iter().flat_map(|v| v.fields.iter()) {
            let Some(from) = field.attrs.iter().find(|a| a.path().is_ident("from")) else {
                continue;
            };
            let holders = held.iter().filter(|ty| ***ty == field.ty).count();
            if holders > 1 {
                self.flag(from.pound_token.span, &rules::FROM_ONCE);
            }
        }
    }

    fn check_use_tree(&mut self, tree: &UseTree, prefix: &mut Vec<String>) {
        match tree {
            UseTree::Path(p) => {
                prefix.push(p.ident.to_string());
                self.check_use_tree(&p.tree, prefix);
                prefix.pop();
            }
            UseTree::Glob(g) if prefix.as_slice() != ["dioxus", "prelude"] => {
                self.flag(g.star_token.span, &rules::GLOB);
            }
            UseTree::Group(g) => {
                for tree in &g.items {
                    self.check_use_tree(tree, prefix);
                }
            }
            _ => {}
        }
    }
}

enum Derived<'v> {
    Struct(&'v Fields),
    Enum(&'v Punctuated<syn::Variant, syn::Token![,]>),
}

impl<'ast> Visit<'ast> for Checker<'_> {
    fn visit_file(&mut self, file: &'ast syn::File) {
        if self.kind.test {
            self.mark_cfg_hosts(&file.attrs, None);
        }
        for attr in &file.attrs {
            let lint = attr_tokens(attr);
            if self.kind.crate_root
                && attr.path().is_ident("allow")
                && lint == "clippy::useless_format"
            {
                self.root_allows.push(std::ptr::from_ref(attr));
            }
        }
        visit::visit_file(self, file);
    }

    fn visit_item(&mut self, item: &'ast Item) {
        match item {
            Item::Fn(i) => self.mark_cfg_hosts(
                &i.attrs,
                Some(CfgItem::new(CfgKind::Fn, i.sig.ident.to_string())),
            ),
            Item::Impl(i) => {
                self.mark_cfg_hosts(&i.attrs, Some(CfgItem::new(CfgKind::Impl, impl_name(i))))
            }
            Item::Use(i) => {
                self.mark_cfg_hosts(&i.attrs, Some(CfgItem::new(CfgKind::Use, String::new())))
            }
            Item::Mod(i) => self.mark_cfg_hosts(&i.attrs, None),
            _ => {}
        }
        visit::visit_item(self, item);
    }

    fn visit_impl_item(&mut self, item: &'ast ImplItem) {
        if let ImplItem::Fn(i) = item {
            self.mark_cfg_hosts(
                &i.attrs,
                Some(CfgItem::new(CfgKind::Fn, i.sig.ident.to_string())),
            );
        }
        visit::visit_impl_item(self, item);
    }

    fn visit_item_fn(&mut self, i: &'ast syn::ItemFn) {
        self.check_fn_attrs(&i.attrs, &i.sig);
        self.mark_ask(&i.vis, &i.sig);
        if let (true, ReturnType::Type(_, ty)) = (i.sig.ident == "main", &i.sig.output) {
            self.set_type(ty, Slot::MainReturn);
        }
        self.enter_fn(&i.sig, &i.block);
        visit::visit_item_fn(self, i);
        self.fns.pop();
    }

    fn visit_impl_item_fn(&mut self, i: &'ast syn::ImplItemFn) {
        self.check_fn_attrs(&i.attrs, &i.sig);
        self.mark_ask(&i.vis, &i.sig);
        self.enter_fn(&i.sig, &i.block);
        visit::visit_impl_item_fn(self, i);
        self.fns.pop();
    }

    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        let foreign = i
            .trait_
            .as_ref()
            .is_some_and(|(_, path, _)| !self.facts.is_ours(path));
        if foreign {
            self.mark(i.span(), Fixed::Always);
        }
        let kind = match &i.trait_ {
            None => ImplKind::Inherent,
            Some((_, path, _)) => {
                let name = path
                    .segments
                    .last()
                    .map(|s| s.ident.to_string())
                    .unwrap_or_default();
                match LIBRARY_TRAITS.contains(&name.as_str()) {
                    true => ImplKind::Library,
                    false => {
                        self.flag(i.impl_token.span, &rules::IMPL_TRAIT);
                        ImplKind::OtherTrait
                    }
                }
            }
        };
        self.impls.push(ImplCtx {
            self_ty: facts::type_name(&i.self_ty),
            kind,
            foreign,
        });
        visit::visit_item_impl(self, i);
        self.impls.pop();
    }

    fn visit_item_struct(&mut self, i: &'ast syn::ItemStruct) {
        self.check_derived(&i.attrs, &i.ident, Derived::Struct(&i.fields));
        self.check_ord(&i.attrs, facts::is_newtype(&i.fields));
        let clap = i
            .attrs
            .iter()
            .any(|a| derives(a, "Parser") || derives(a, "clap::Parser"));
        let crate_visible = is_pub_crate(&i.vis);
        let row = self.tables.is_row(i);
        for field in &i.fields {
            if clap {
                self.set_type(&field.ty, Slot::ClapField);
            }
            if row {
                self.set_type(&field.ty, Slot::RowField);
            }
            if let (true, Visibility::Public(token)) = (crate_visible, &field.vis) {
                self.flag(token.span, &rules::PUB_FIELD);
            }
        }
        visit::visit_item_struct(self, i);
    }

    fn visit_item_enum(&mut self, i: &'ast syn::ItemEnum) {
        self.check_derived(&i.attrs, &i.ident, Derived::Enum(&i.variants));
        self.check_ord(&i.attrs, false);
        self.check_from(&i.variants);
        visit::visit_item_enum(self, i);
    }

    fn visit_item_const(&mut self, i: &'ast syn::ItemConst) {
        self.check_tuple_rows(&i.ty);
        self.set_type(&i.ty, Slot::ConstType);
        self.set_expr(&i.expr, Slot::ConstInit);
        visit::visit_item_const(self, i);
    }

    fn visit_impl_item_const(&mut self, i: &'ast syn::ImplItemConst) {
        self.check_tuple_rows(&i.ty);
        self.set_type(&i.ty, Slot::ConstType);
        self.set_expr(&i.expr, Slot::ConstInit);
        visit::visit_impl_item_const(self, i);
    }

    fn visit_item_use(&mut self, i: &'ast syn::ItemUse) {
        self.check_use_tree(&i.tree, &mut Vec::new());
        for path in use_paths(&i.tree).iter().filter(|p| p.name.is_some()) {
            if let Some(rule) = self.named_rule(&path.segments, &USE_RULES) {
                self.flag(path.span, rule);
            }
        }
        visit::visit_item_use(self, i);
    }

    fn visit_signature(&mut self, sig: &'ast syn::Signature) {
        self.sig_generics = Some(std::ptr::from_ref(&sig.generics));
        for arg in &sig.inputs {
            if let (FnArg::Typed(t), false) = (
                arg,
                self.ask_generics == Some(std::ptr::from_ref(&sig.generics)),
            ) {
                self.set_type(&t.ty, Slot::Param);
            }
        }
        self.visit_generics(&sig.generics);
        for arg in &sig.inputs {
            self.visit_fn_arg(arg);
        }
        let outer = std::mem::replace(&mut self.in_return, true);
        self.visit_return_type(&sig.output);
        self.in_return = outer;
    }

    fn visit_generics(&mut self, generics: &'ast syn::Generics) {
        let on_fn = self.sig_generics == Some(std::ptr::from_ref(generics));
        let ask = self.ask_generics == Some(std::ptr::from_ref(generics));
        let library = self.in_library_impl();
        let foreign = self.in_foreign_impl();
        for param in &generics.params {
            let library_bounds = match param {
                GenericParam::Type(t) => !t.bounds.is_empty()
                    && t.bounds.iter().all(
                        |b| matches!(b, TypeParamBound::Trait(b) if !self.facts.is_ours(&b.path)),
                    ),
                _ => false,
            };
            if foreign || library_bounds {
                self.mark(param.span(), Fixed::Always);
            }
            match param {
                GenericParam::Type(t) => {
                    let codec = on_fn
                        && generics.where_clause.is_none()
                        && matches!(
                            t.bounds.iter().collect::<Vec<_>>().as_slice(),
                            [TypeParamBound::Trait(b)] if b.path.segments.last().is_some_and(|s| CODEC_BOUNDS.contains(&s.ident.to_string().as_str()))
                        );
                    if !(library || codec || ask) {
                        self.flag(param.span(), &rules::TYPE_PARAM);
                    }
                }
                GenericParam::Lifetime(_) if !library => {
                    self.flag(param.span(), &rules::LIFETIME_PARAM);
                }
                _ => {}
            }
        }
        visit::visit_generics(self, generics);
    }

    fn visit_lifetime_param(&mut self, _: &'ast syn::LifetimeParam) {}

    fn visit_label(&mut self, _: &'ast syn::Label) {}

    fn visit_lifetime(&mut self, lifetime: &'ast syn::Lifetime) {
        if self.static_strs.contains(&std::ptr::from_ref(lifetime)) {
            return;
        }
        if self.in_foreign_impl() {
            self.mark(lifetime.span(), Fixed::Always);
        }
        if !self.in_library_impl() {
            self.flag(lifetime.span(), &rules::LIFETIME);
        }
    }

    fn visit_visibility(&mut self, vis: &'ast Visibility) {
        if let (Visibility::Restricted(r), false) = (vis, is_pub_crate(vis)) {
            self.flag(r.pub_token.span, &rules::VIS_CRATE);
        }
    }

    fn visit_attribute(&mut self, attr: &'ast Attribute) {
        let path = path_string(attr.path());
        let library = match path.as_str() {
            "derive" => derive_names(attr)
                .iter()
                .any(|name| !crate::context::STD_DERIVES.contains(&name.as_str())),
            _ => !BUILTIN_ATTRS.contains(&path.as_str()),
        };
        if library {
            self.mark(attr.span(), Fixed::Always);
        }
        match path.as_str() {
            "cfg" if !is_cfg_test(attr) => self.check_cfg(attr),
            "allow" if !self.root_allows.contains(&std::ptr::from_ref(attr)) => {
                self.flag(attr.pound_token.span, &rules::ALLOW);
            }
            "serde" => {
                let keys = meta_keys(attr);
                if keys.iter().any(|k| !SERDE_KEYS.contains(&k.as_str())) {
                    self.flag(attr.pound_token.span, &rules::SERDE_KEYS);
                }
            }
            "arg" if attr_tokens(attr).contains("value_parser=[") => {
                self.flag(attr.pound_token.span, &rules::VALUE_PARSER);
            }
            _ => {}
        }
    }

    fn visit_path_segment(&mut self, segment: &'ast syn::PathSegment) {
        if let PathArguments::AngleBracketed(args) = &segment.arguments {
            let name = segment.ident.to_string();
            // Our `Result` aliases only fill in std's error type, so their
            // `Result<(), E>` is still 0005's.
            let ours = name != "Result"
                && (self.facts.types.contains(&name) || self.facts.fns.contains(&name));
            for arg in &args.args {
                let ty = match arg {
                    GenericArgument::Type(ty) => ty,
                    GenericArgument::AssocType(binding) => &binding.ty,
                    _ => continue,
                };
                let slot = Slot::GenericArg {
                    segment: name.clone(),
                    ours,
                };
                self.set_type(ty, slot);
            }
        }
        visit::visit_path_segment(self, segment);
    }

    fn visit_type(&mut self, ty: &'ast Type) {
        let slot = self.type_slot(ty);
        if let Type::Path(t) = ty
            && t.qself.is_none()
            && let Some(rule) = self.named_rule(&segments(&t.path), &TYPE_RULES)
        {
            self.flag(t.span(), rule);
        }
        if library_type_arg(&slot) || matches!(slot, Slot::ClapField | Slot::MainReturn) {
            self.mark(ty.span(), Fixed::IfRestricted);
        }
        if let Type::Reference(t) = ty
            && t.mutability.is_some()
            && !matches!(&*t.elem, Type::Path(p) if self.facts.is_ours(&p.path))
        {
            self.mark(ty.span(), Fixed::Always);
        }
        match ty {
            Type::Tuple(t) if t.elems.is_empty() => {
                if !(library_type_arg(&slot) || slot == Slot::MainReturn) {
                    self.flag(t.span(), &rules::UNIT_TYPE);
                }
            }
            Type::Reference(t) => {
                if let (Some(lifetime), true) = (&t.lifetime, table::is_static_str(ty)) {
                    self.static_strs.insert(std::ptr::from_ref(lifetime));
                    if slot != Slot::RowField {
                        self.flag(lifetime.span(), &rules::STATIC_STR);
                    }
                }
                if let (Some(token), Type::Path(p)) = (t.mutability, &*t.elem)
                    && self.facts.is_ours(&p.path)
                {
                    self.flag(token.span, &rules::REF_MUT_OURS);
                }
                self.set_type(&t.elem, Slot::RefTarget(Box::new(slot)));
            }
            Type::Slice(t) => {
                let read_only = matches!(&slot, Slot::RefTarget(outer) if matches!(**outer, Slot::Param | Slot::ConstType));
                if !read_only {
                    self.flag(t.span(), &rules::SLICE);
                }
            }
            Type::Array(t) => {
                let bytes = matches!(&*t.elem, Type::Path(p) if p.path.is_ident("u8"));
                if !bytes {
                    self.flag(t.span(), &rules::ARRAY_TYPE);
                }
            }
            Type::ImplTrait(t) if !self.in_return && slot != Slot::AskBuild => {
                self.flag(t.impl_token.span, &rules::IMPL_TRAIT_ARG);
            }
            Type::TraitObject(t) => {
                let fn_mut = matches!(
                    t.bounds.iter().collect::<Vec<_>>().as_slice(),
                    [TypeParamBound::Trait(b)] if b.path.segments.last().is_some_and(|s| s.ident == "FnMut")
                );
                let in_closure =
                    matches!(&slot, Slot::GenericArg { segment, .. } if segment == "Closure");
                if !(fn_mut && in_closure) {
                    self.flag(t.span(), &rules::DYN);
                }
            }
            Type::Path(t)
                if t.qself.is_none()
                    && t.path.is_ident("bool")
                    && !(library_type_arg(&slot) || slot == Slot::ClapField) =>
            {
                self.flag(t.span(), &rules::BOOL_TYPE);
            }
            _ => {}
        }
        visit::visit_type(self, ty);
    }

    fn visit_expr(&mut self, expr: &'ast Expr) {
        let slot = self.expr_slot(expr);
        match expr {
            Expr::Path(e) if e.qself.is_none() => {
                if let Some(rule) = self.named_rule(&segments(&e.path), &PATH_RULES) {
                    self.flag(e.span(), rule);
                }
            }
            Expr::MethodCall(e) if self.operator_methods.contains(&e.method.to_string()) => {
                self.flag(e.method.span(), &rules::OPERATOR_METHOD);
            }
            _ => {}
        }
        if library_value(&slot) {
            self.mark(expr.span(), Fixed::IfRestricted);
        }
        match expr {
            Expr::Call(e) => {
                let callee = match &*e.func {
                    Expr::Path(p) => Callee {
                        path: p
                            .path
                            .segments
                            .iter()
                            .map(|s| s.ident.to_string())
                            .collect(),
                        ours: self.facts.is_ours(&p.path),
                    },
                    _ => Callee {
                        path: Vec::new(),
                        ours: false,
                    },
                };
                if callee.path.len() >= 2
                    && COMPARE_TRAITS.contains(&callee.path[callee.path.len() - 2].as_str())
                    && (COMPARE_METHODS.contains(&callee.last()) || callee.last() == "cmp")
                    && self.in_predicate == 0
                {
                    self.flag(e.func.span(), &rules::LIFT_EQ);
                }
                let drop = matches!(&*e.func, Expr::Path(p) if is_drop(&p.path));
                let named =
                    matches!(e.args.first(), Some(Expr::Path(p)) if p.path.get_ident().is_some());
                if drop && !named {
                    self.flag(e.func.span(), &rules::DROP_DISCARD);
                }
                let range_fn = callee.path.len() >= 2
                    && RANGE_TYPES.contains(&callee.path[callee.path.len() - 2].as_str())
                    && !self.range_sources.contains(&std::ptr::from_ref(expr));
                let bound = BOUNDS.contains(&callee.last());
                if (range_fn || bound) && !callee.ours {
                    self.flag(e.func.span(), &rules::RANGE_CALL);
                }
                for arg in &e.args {
                    self.set_expr(arg, Slot::CallArg(callee.clone()));
                }
            }
            Expr::MethodCall(e) => {
                if e.method == "cmp" && self.ordered_newtype_field(&e.receiver) {
                    self.flag(e.method.span(), &rules::ORD_INNER);
                }
                if e.method == "count"
                    && e.args.is_empty()
                    && matches!(&*e.receiver, Expr::MethodCall(m) if m.method == "filter_map")
                {
                    self.flag(e.method.span(), &rules::COUNT);
                }
                let some_bindings = self
                    .fns
                    .last()
                    .map(|f| f.some_bindings.clone())
                    .unwrap_or_default();
                if ["filter", "find"].contains(&e.method.to_string().as_str())
                    && built_option(&e.receiver, &some_bindings)
                    && matches!(e.args.first(), Some(Expr::Closure(c)) if comparison(&c.body))
                {
                    self.flag(e.method.span(), &rules::LIFT_FILTER);
                }
                let by_name =
                    COMPARE_METHODS.contains(&e.method.to_string().as_str()) && e.args.len() == 1;
                let ordering = ORDERING_TESTS.contains(&e.method.to_string().as_str())
                    && matches!(&*e.receiver, Expr::MethodCall(m) if m.method == "cmp" || m.method == "partial_cmp");
                if (by_name || ordering) && self.in_predicate == 0 {
                    self.flag(e.method.span(), &rules::LIFT_EQ);
                }
                if e.method == "for_each" || e.method == "try_for_each" {
                    self.flag(e.method.span(), &rules::FOR_EACH);
                }
                let streams = self
                    .fns
                    .last()
                    .map(|f| f.stream_bindings.clone())
                    .unwrap_or_default();
                let kept = e
                    .turbofish
                    .as_ref()
                    .is_some_and(|turbofish| !names_unit(turbofish));
                if e.method == "try_collect" && stream_root(&e.receiver, &streams) && !kept {
                    self.flag(e.method.span(), &rules::STREAM_EFFECTS);
                }
                if e.method == "map"
                    && matches!(e.args.first(), Some(Expr::Closure(c)) if matches!(&*c.body, Expr::Tuple(t) if t.elems.is_empty()))
                    && matches!(&*e.receiver, Expr::Await(a) if matches!(&*a.base, Expr::MethodCall(m) if m.method == "try_collect" && stream_root(&m.receiver, &streams)))
                {
                    self.flag(e.method.span(), &rules::STREAM_EFFECTS);
                }
                // `into_iter` on a range returns the range itself, so it
                // iterates only when its own result is iterated.
                let into_iter_iterated = e.method == "into_iter"
                    && self.range_sources.contains(&std::ptr::from_ref(expr));
                if self.lists.iterates(&e.method.to_string())
                    || into_iter_iterated
                    || e.method == "contains"
                {
                    self.range_sources
                        .insert(std::ptr::from_ref(unparen(&e.receiver)));
                }
                if e.method == "split_at" || e.method == "split_at_mut" {
                    self.flag(e.method.span(), &rules::SPLIT_AT);
                }
                if e.method == "zip" || e.method == "chain" {
                    for arg in &e.args {
                        self.range_sources.insert(std::ptr::from_ref(unparen(arg)));
                    }
                }
                let bindings = self
                    .fns
                    .last()
                    .map(|f| f.table_bindings.clone())
                    .unwrap_or_default();
                if (self.lists.predicate(&e.method.to_string()) || e.method == "rposition")
                    && let (Some(scan), Some(Expr::Closure(c))) =
                        (self.tables.scanned(&e.receiver, &bindings), e.args.first())
                    && table::keyed(c, &scan)
                {
                    self.flag(e.method.span(), &rules::KEYED_TABLE);
                }
                let on_self = matches!(&*e.receiver, Expr::Path(p) if p.path.is_ident("self"));
                for arg in &e.args {
                    let slot = Slot::MethodArg {
                        method: e.method.to_string(),
                        on_self,
                    };
                    self.set_expr(arg, slot);
                }
            }
            Expr::Struct(e) => {
                let ours = self.facts.is_ours(&e.path);
                if !ours
                    && e.path
                        .segments
                        .last()
                        .is_some_and(|s| RANGE_TYPES.contains(&s.ident.to_string().as_str()))
                    && !self.range_sources.contains(&std::ptr::from_ref(expr))
                {
                    self.flag(e.path.span(), &rules::RANGE_STRUCT);
                }
                for field in &e.fields {
                    self.set_expr(&field.expr, Slot::FieldValue { ours });
                }
                if let Some(rest) = &e.rest
                    && !matches!(**rest, Expr::Path(_) | Expr::Call(_) | Expr::MethodCall(_))
                {
                    self.flag(e.span(), &rules::STRUCT_UPDATE);
                }
            }
            Expr::Closure(e) => {
                if let Slot::MethodArg { method, .. } = &slot
                    && self.lists.predicate(method)
                {
                    self.mark_predicate(&e.body);
                }
                if let Slot::MethodArg { method, .. } = &slot
                    && !self.lists.takes_closure(method)
                {
                    self.flag(e.span(), &rules::CLOSURE_METHOD);
                }
                let known = match &slot {
                    Slot::MethodArg { .. } => true,
                    Slot::CallArg(callee) => {
                        callee.ends_with(&["Closure", "new"])
                            || CLOSURE_CALLEES.contains(&callee.last())
                    }
                    _ => false,
                };
                if !known {
                    self.flag(e.span(), &rules::CLOSURE);
                }
                self.set_expr(&e.body, Slot::ClosureBody(Box::new(slot.clone())));
                if let Slot::MethodArg { method, .. } = &slot
                    && self.lists.predicate(method)
                {
                    self.in_predicate += 1;
                    visit::visit_expr(self, expr);
                    self.in_predicate -= 1;
                    return;
                }
            }
            Expr::Async(e) => {
                let placed = match &slot {
                    Slot::CallArg(callee) => callee.last() == "spawn",
                    Slot::ClosureBody(outer) => match &**outer {
                        Slot::CallArg(callee) => ["unfold", "use_future"].contains(&callee.last()),
                        Slot::MethodArg { method, .. } => method == "then",
                        _ => false,
                    },
                    _ => false,
                };
                if !placed {
                    self.flag(e.async_token.span, &rules::ASYNC_BLOCK);
                }
            }
            Expr::Path(e)
                if matches!(slot, Slot::CallArg(_) | Slot::MethodArg { .. })
                    && is_drop(&e.path) =>
            {
                self.flag(e.span(), &rules::DROP_FN);
            }
            Expr::Lit(e) if matches!(e.lit, Lit::Bool(_)) && !library_value(&slot) => {
                self.flag(e.span(), &rules::BOOL_LIT);
            }
            Expr::Tuple(e) if e.elems.is_empty() && !library_value(&slot) => {
                self.flag(e.span(), &rules::UNIT_VALUE);
            }
            Expr::Path(e)
                if e.path
                    .segments
                    .last()
                    .is_some_and(|s| s.ident == "RangeFull" || s.ident == "Unbounded")
                    && !self.facts.is_ours(&e.path) =>
            {
                self.flag(e.span(), &rules::RANGE_PATH);
            }
            // `&[a, b]` stands where its array would: `f(&[a, b])` passes the
            // list straight to a call, and `const L: &[T] = &[..]` is a const.
            Expr::Reference(e)
                if e.mutability.is_none()
                    && matches!(
                        slot,
                        Slot::CallArg(_) | Slot::MethodArg { .. } | Slot::ConstInit
                    ) =>
            {
                self.set_expr(&e.expr, slot.clone());
            }
            Expr::Array(e) => {
                if e.elems.len() >= 2
                    && e.elems
                        .iter()
                        .any(|row| matches!(row, Expr::Tuple(_)) && leaves(row) > 2)
                {
                    self.flag(e.span(), &rules::TUPLE_TABLE);
                }
                if !matches!(
                    slot,
                    Slot::CallArg(_) | Slot::MethodArg { .. } | Slot::ConstInit
                ) {
                    self.flag(e.span(), &rules::ARRAY_VALUE);
                }
            }
            Expr::Binary(e) if !self.predicates.contains(&std::ptr::from_ref(expr)) => {
                if let Some(rule) = predicate_rule(&e.op) {
                    self.flag(e.op.span(), rule);
                }
            }
            Expr::Unary(e)
                if matches!(e.op, syn::UnOp::Not(_))
                    && !self.predicates.contains(&std::ptr::from_ref(expr)) =>
            {
                self.flag(e.op.span(), &rules::PREDICATE_NOT);
            }
            Expr::Repeat(e) => {
                if !matches!(*e.expr, Expr::Lit(_)) {
                    self.flag(e.span(), &rules::REPEAT);
                }
            }
            Expr::Loop(e) => {
                let actor = self
                    .fns
                    .last()
                    .and_then(|f| f.actor_loop)
                    .is_some_and(|l| std::ptr::eq(l, e));
                match (actor, actor::turn_is_simple(&e.body)) {
                    (false, _) => self.flag(e.loop_token.span, &rules::LOOP_POSITION),
                    (true, false) => self.flag(e.loop_token.span, &rules::LOOP_TURN),
                    (true, true) => {}
                }
                self.loops.push(actor);
                visit::visit_expr(self, expr);
                self.loops.pop();
                return;
            }
            Expr::Match(e) if filtered_once(&e.expr) => {
                self.flag(e.match_token.span, &rules::LIFT_MATCHED);
            }
            Expr::If(e) if matches!(&*e.cond, Expr::Let(l) if filtered_once(&l.expr)) => {
                self.flag(e.if_token.span, &rules::LIFT_MATCHED);
            }
            Expr::ForLoop(e) => {
                self.range_sources
                    .insert(std::ptr::from_ref(unparen(&e.expr)));
                if e.body.stmts.is_empty() || !e.body.stmts.iter().all(effect_stmt) {
                    self.flag(e.for_token.span, &rules::FOR_BODY);
                }
                if !value_free(&e.expr, ValueFree::Iterator) {
                    self.flag(e.for_token.span, &rules::FOR_ITER);
                }
                self.loops.push(false);
                visit::visit_expr(self, expr);
                self.loops.pop();
                return;
            }
            Expr::While(_) => {
                self.loops.push(false);
                visit::visit_expr(self, expr);
                self.loops.pop();
                return;
            }
            Expr::Break(e) => {
                if !(self.loops.last() == Some(&true) && e.expr.is_some()) {
                    self.flag(e.break_token.span, &rules::BREAK);
                }
            }
            Expr::Assign(e) => {
                let ctx = self.fns.last();
                let to_self = ctx
                    .is_some_and(|f| matches!(f.receiver, Receiver::RefMut | Receiver::OwnedMut))
                    && field_of_self(&e.left);
                let to_state = matches!(&*e.left, Expr::Path(p) if ctx.is_some_and(|f| p.path.get_ident().is_some_and(|i| f.state_names.contains(&i.to_string()))));
                if !(to_self || to_state) {
                    self.flag(e.eq_token.span, &rules::ASSIGN);
                }
            }
            _ => {}
        }
        visit::visit_expr(self, expr);
    }

    fn visit_expr_match(&mut self, m: &'ast syn::ExprMatch) {
        for (i, arm) in m.arms.iter().enumerate() {
            let Some((token, guard)) = &arm.guard else {
                continue;
            };
            let bound = pattern_bindings(&arm.pat);
            match guard.as_ref() {
                Expr::Binary(e) if comparison_op(&e.op) => {
                    self.predicates.insert(std::ptr::from_ref(guard.as_ref()));
                    if !(rooted_in(&e.left, &bound) || rooted_in(&e.right, &bound)) {
                        self.flag(token.span, &rules::GUARD_BOUND);
                    }
                    if constant(&e.left) || constant(&e.right) {
                        self.flag(token.span, &rules::GUARD_PATTERN);
                    }
                }
                Expr::Macro(e) if e.mac.path.is_ident("matches") => {
                    self.predicate_macros.insert(std::ptr::from_ref(&e.mac));
                    if let Some(parsed) = parse_matches(&e.mac) {
                        if !rooted_in(&parsed.scrutinee, &bound) {
                            self.flag(token.span, &rules::GUARD_BOUND);
                        }
                        if !calls(&parsed.scrutinee) {
                            self.flag(token.span, &rules::GUARD_PATTERN);
                        }
                    }
                }
                _ => self.flag(token.span, &rules::GUARD),
            }
            let fallback = m.arms[i + 1..]
                .iter()
                .any(|later| later.guard.is_none() && covers(&later.pat, &arm.pat));
            if !fallback {
                self.flag(token.span, &rules::GUARD_FALLBACK);
            }
        }
        visit::visit_expr_match(self, m);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        if !STD_MACROS.contains(&path_string(&mac.path).as_str()) {
            self.mark(mac.span(), Fixed::Always);
        }
        if mac.path.is_ident("matches") && !self.predicate_macros.contains(&std::ptr::from_ref(mac))
        {
            self.flag(mac.path.span(), &rules::PREDICATE_MATCHES);
        }
        if mac.path.is_ident("matches")
            && self.predicate_macros.contains(&std::ptr::from_ref(mac))
            && !parse_matches(mac).is_some_and(|m| m.plain())
        {
            self.flag(mac.path.span(), &rules::MATCHES_FORM);
        }
        visit::visit_macro(self, mac);
    }

    fn visit_stmt(&mut self, stmt: &'ast syn::Stmt) {
        if let syn::Stmt::Expr(Expr::MethodCall(e), Some(_)) = stmt
            && e.method == "ok"
            && e.args.is_empty()
        {
            self.flag(e.method.span(), &rules::OK_DISCARD);
        }
        visit::visit_stmt(self, stmt);
    }

    fn visit_local(&mut self, local: &'ast syn::Local) {
        let names = actor::mut_bindings(&local.pat);
        let state = self
            .fns
            .last()
            .and_then(|f| f.state)
            .is_some_and(|s| std::ptr::eq(s, local));
        if !names.is_empty() && !state && self.assigned_here(&names) {
            self.flag(local.let_token.span, &rules::LET_MUT);
        }
        let (binding, ty) = match &local.pat {
            syn::Pat::Type(t) => (&*t.pat, Some(&*t.ty)),
            pat => (pat, None),
        };
        let init = local.init.as_ref().map(|i| &*i.expr);
        if let syn::Pat::Ident(p) = binding
            && p.mutability.is_some()
            && !state
            && self.mut_of_ours(ty, init).is_some()
        {
            self.flag(local.let_token.span, &rules::LET_MUT_OURS);
        }
        if !names.is_empty() && !state {
            self.mark(local.span(), Fixed::Always);
        }
        for attr in &local.attrs {
            self.visit_attribute(attr);
        }
        self.in_let = true;
        self.visit_pat(&local.pat);
        self.in_let = false;
        if let Some(init) = &local.init {
            self.visit_local_init(init);
        }
    }

    fn visit_pat_ident(&mut self, p: &'ast syn::PatIdent) {
        if p.mutability.is_some() && !self.in_let && self.assigned_here(&[p.ident.to_string()]) {
            self.flag(p.span(), &rules::PAT_MUT);
        }
        if p.mutability.is_some() && !self.in_let {
            self.mark(p.span(), Fixed::Always);
        }
        visit::visit_pat_ident(self, p);
    }
}

fn comparison_op(op: &syn::BinOp) -> bool {
    matches!(
        op,
        syn::BinOp::Eq(_)
            | syn::BinOp::Ne(_)
            | syn::BinOp::Lt(_)
            | syn::BinOp::Le(_)
            | syn::BinOp::Gt(_)
            | syn::BinOp::Ge(_)
    )
}

/// A closure body that is one comparison, through parentheses, `!` and a
/// block holding only its tail.
fn comparison(body: &Expr) -> bool {
    match body {
        Expr::Paren(e) => comparison(&e.expr),
        Expr::Unary(e) if matches!(e.op, syn::UnOp::Not(_)) => comparison(&e.expr),
        Expr::Block(e) if e.label.is_none() => {
            matches!(e.block.stmts.as_slice(), [syn::Stmt::Expr(tail, None)] if comparison(tail))
        }
        Expr::Binary(e) => comparison_op(&e.op),
        _ => false,
    }
}

/// The std range types, whose construction by name is a range expression.
const RANGE_TYPES: &[&str] = &[
    "Range",
    "RangeInclusive",
    "RangeFrom",
    "RangeTo",
    "RangeToInclusive",
    "RangeFull",
];
/// The variants of `std::ops::Bound`, whose construction builds a range.
const BOUNDS: &[&str] = &["Included", "Excluded", "Unbounded"];

fn unparen(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(e) => unparen(&e.expr),
        _ => expr,
    }
}

/// The fields of a row, counted through nested tuples: `(a, (b, c))` has
/// three.
fn leaves(expr: &Expr) -> usize {
    match expr {
        Expr::Tuple(t) => t.elems.iter().map(leaves).sum(),
        _ => 1,
    }
}

/// Methods that compare by name, as `a.eq(&b)` (proposal 0040).
const COMPARE_METHODS: &[&str] = &["eq", "ne", "lt", "le", "gt", "ge"];
/// `Ordering`'s tests, as in `a.cmp(&b).is_eq()`.
const ORDERING_TESTS: &[&str] = &["is_eq", "is_ne", "is_lt", "is_le", "is_gt", "is_ge"];
const COMPARE_TRAITS: &[&str] = &["PartialEq", "PartialOrd", "Ord"];

/// `Some(x)`, an `Option` built at the call, by any path (proposal 0040).
fn some_call(expr: &Expr) -> bool {
    matches!(expr, Expr::Call(c) if matches!(&*c.func, Expr::Path(p)
        if p.path.segments.last().is_some_and(|s| s.ident == "Some")))
}

/// An `Option` built only to ask one question: `Some(x)`, a name bound to
/// one in this fn, or either through `into_iter()` or `iter()`.
fn built_option(expr: &Expr, some_bindings: &BTreeSet<String>) -> bool {
    match expr {
        Expr::MethodCall(m) if m.method == "into_iter" || m.method == "iter" => {
            built_option(&m.receiver, some_bindings)
        }
        Expr::Path(p) => p
            .path
            .get_ident()
            .is_some_and(|i| some_bindings.contains(&i.to_string())),
        Expr::Paren(e) => built_option(&e.expr, some_bindings),
        other => some_call(other),
    }
}

/// `opt.filter(|x| <one comparison>)` as a `match` or `if let` scrutinee:
/// the owner's ruling makes it a lift too.
fn filtered_once(expr: &Expr) -> bool {
    matches!(expr, Expr::MethodCall(m) if m.method == "filter"
        && matches!(m.args.first(), Some(Expr::Closure(c)) if comparison(&c.body)))
}

/// A chain of method calls that starts at `stream::iter(..)`, a bare `iter(..)`
/// imported from a stream module, or a name bound to such a chain.
fn stream_root(expr: &Expr, streams: &BTreeSet<String>) -> bool {
    match expr {
        Expr::MethodCall(m) => stream_root(&m.receiver, streams),
        Expr::Await(a) => stream_root(&a.base, streams),
        Expr::Paren(e) => stream_root(&e.expr, streams),
        Expr::Call(c) => matches!(&*c.func, Expr::Path(p) if {
            let path = segments(&p.path);
            path.ends_with(&["stream".to_string(), "iter".to_string()]) || path == ["iter"]
        }),
        Expr::Path(p) => p
            .path
            .get_ident()
            .is_some_and(|i| streams.contains(&i.to_string())),
        _ => false,
    }
}

/// The names a fn body binds with `let name = <init>` where `test` holds for
/// the init, in order, so a later binding can build on an earlier one.
fn bound_to(block: &syn::Block, test: fn(&Expr, &BTreeSet<String>) -> bool) -> BTreeSet<String> {
    struct Finder {
        test: fn(&Expr, &BTreeSet<String>) -> bool,
        names: BTreeSet<String>,
    }
    impl<'ast> Visit<'ast> for Finder {
        fn visit_local(&mut self, local: &'ast syn::Local) {
            if let (syn::Pat::Ident(p), Some(init)) = (&local.pat, &local.init)
                && (self.test)(&init.expr, &self.names)
            {
                self.names.insert(p.ident.to_string());
            }
            visit::visit_local(self, local);
        }
        fn visit_item(&mut self, _: &'ast Item) {}
    }
    let mut finder = Finder {
        test,
        names: BTreeSet::new(),
    };
    finder.visit_block(block);
    finder.names
}

/// A turbofish that names `()`, as in `try_collect::<()>()` or
/// `try_collect::<Vec<()>>()`.
fn names_unit(turbofish: &syn::AngleBracketedGenericArguments) -> bool {
    struct Unit(bool);
    impl<'ast> Visit<'ast> for Unit {
        fn visit_type_tuple(&mut self, t: &'ast syn::TypeTuple) {
            self.0 |= t.elems.is_empty();
            visit::visit_type_tuple(self, t);
        }
    }
    let mut unit = Unit(false);
    unit.visit_angle_bracketed_generic_arguments(turbofish);
    unit.0
}

/// One statement of a `for` body (proposal 0041 and the owner's ruling): a
/// call, optionally `.await` and `?`, as `f(x);`, `f(x)?;` or
/// `x.m(..).await?;`, or a call discarded with `let _ =` (0025). An `.await`
/// is always followed by `?`, and the call builds no value on the way.
fn effect_stmt(stmt: &syn::Stmt) -> bool {
    match stmt {
        syn::Stmt::Expr(e, Some(_)) => effect_call(e),
        syn::Stmt::Local(local) => {
            matches!(local.pat, syn::Pat::Wild(_))
                && local
                    .init
                    .as_ref()
                    .is_some_and(|init| init.diverge.is_none() && effect_call(&init.expr))
        }
        _ => false,
    }
}

fn effect_call(expr: &Expr) -> bool {
    let call = match expr {
        Expr::Try(t) => match &*t.expr {
            Expr::Await(a) => &*a.base,
            other => other,
        },
        other => other,
    };
    matches!(call, Expr::Call(_) | Expr::MethodCall(_)) && value_free(call, ValueFree::Call)
}

#[derive(Clone, Copy, PartialEq)]
enum ValueFree {
    /// An effect call: no `.await` inside it either, so one call is one step.
    Call,
    /// A `for`'s iterator expression.
    Iterator,
}

/// No block with statements, assignment, `let`, `if`, `match` or loop in
/// `expr`, outside the bodies of the closures it passes to adapters, and no
/// assignment even there.
fn value_free(expr: &Expr, kind: ValueFree) -> bool {
    struct Finder {
        kind: ValueFree,
        closures: usize,
        found: bool,
    }
    impl<'ast> Visit<'ast> for Finder {
        fn visit_expr(&mut self, e: &'ast Expr) {
            let building = match e {
                Expr::Assign(_) | Expr::Let(_) => true,
                Expr::Block(_)
                | Expr::If(_)
                | Expr::Match(_)
                | Expr::Loop(_)
                | Expr::While(_)
                | Expr::ForLoop(_)
                | Expr::Async(_) => self.closures == 0,
                Expr::Await(_) => self.kind == ValueFree::Call,
                _ => false,
            };
            self.found |= building;
            if let Expr::Closure(c) = e {
                self.closures += 1;
                self.visit_expr(&c.body);
                self.closures -= 1;
                return;
            }
            visit::visit_expr(self, e);
        }
        fn visit_item(&mut self, _: &'ast Item) {}
    }
    let mut finder = Finder {
        kind,
        closures: 0,
        found: false,
    };
    finder.visit_expr(expr);
    !finder.found
}

/// The names a pattern binds.
fn pattern_bindings(pat: &syn::Pat) -> BTreeSet<String> {
    struct Finder(BTreeSet<String>);
    impl<'ast> Visit<'ast> for Finder {
        fn visit_pat_ident(&mut self, p: &'ast syn::PatIdent) {
            if binds(&p.ident) {
                self.0.insert(p.ident.to_string());
            }
            visit::visit_pat_ident(self, p);
        }
    }
    let mut finder = Finder(BTreeSet::new());
    finder.visit_pat(pat);
    finder.0
}

/// Whether `expr` is a place or call chain whose root is one of `names`, as
/// `hello.token`, `&call` or `item.kind()`.
fn rooted_in(expr: &Expr, names: &BTreeSet<String>) -> bool {
    match expr {
        Expr::Path(p) => p
            .path
            .get_ident()
            .is_some_and(|i| names.contains(&i.to_string())),
        Expr::Field(e) => rooted_in(&e.base, names),
        Expr::MethodCall(e) => rooted_in(&e.receiver, names),
        Expr::Reference(e) => rooted_in(&e.expr, names),
        Expr::Paren(e) => rooted_in(&e.expr, names),
        Expr::Unary(e) if matches!(e.op, syn::UnOp::Deref(_)) => rooted_in(&e.expr, names),
        _ => false,
    }
}

/// A value a pattern can name: a literal, a unit variant or constant, or a
/// constructor of such values (bootstrap review §A3, §A4).
fn constant(expr: &Expr) -> bool {
    match expr {
        Expr::Lit(_) => true,
        Expr::Path(p) => p
            .path
            .segments
            .last()
            .is_some_and(|s| s.ident.to_string().starts_with(char::is_uppercase)),
        Expr::Reference(e) => constant(&e.expr),
        Expr::Paren(e) => constant(&e.expr),
        Expr::Call(c) => constant(&c.func) && c.args.iter().all(constant),
        Expr::Tuple(t) => !t.elems.is_empty() && t.elems.iter().all(constant),
        Expr::Struct(s) => s.rest.is_none() && s.fields.iter().all(|f| constant(&f.expr)),
        _ => false,
    }
}

/// Whether a call happens in `expr`: what a pattern can't express.
fn calls(expr: &Expr) -> bool {
    match expr {
        Expr::Call(_) | Expr::MethodCall(_) => true,
        Expr::Field(e) => calls(&e.base),
        Expr::Reference(e) => calls(&e.expr),
        Expr::Paren(e) => calls(&e.expr),
        _ => false,
    }
}

/// The parts of `matches!(scrutinee, pattern if guard)`.
struct Matches {
    scrutinee: Expr,
    pattern: syn::Pat,
    guard: Option<Expr>,
}

impl Matches {
    /// No inner guard, no `true` / `false` pattern and a scrutinee that is a
    /// place or a call: nothing carries a boolean through the macro
    /// (proposals 0028, 0040).
    fn plain(&self) -> bool {
        struct BoolPat(bool);
        impl<'ast> Visit<'ast> for BoolPat {
            fn visit_lit_bool(&mut self, _: &'ast syn::LitBool) {
                self.0 = true;
            }
        }
        let mut bools = BoolPat(false);
        bools.visit_pat(&self.pattern);
        self.guard.is_none() && !bools.0 && place_or_call(&self.scrutinee)
    }
}

fn parse_matches(mac: &syn::Macro) -> Option<Matches> {
    use syn::parse::Parser;
    let parser = |input: syn::parse::ParseStream| {
        let scrutinee: Expr = input.parse()?;
        input.parse::<syn::Token![,]>()?;
        let pattern = syn::Pat::parse_multi_with_leading_vert(input)?;
        let guard = match input.parse::<Option<syn::Token![if]>>()? {
            Some(_) => Some(input.parse::<Expr>()?),
            None => None,
        };
        let _ = input.parse::<Option<syn::Token![,]>>()?;
        Ok(Matches {
            scrutinee,
            pattern,
            guard,
        })
    };
    parser.parse2(mac.tokens.clone()).ok()
}

/// A place, a literal or a call whose receiver and arguments are such, or a
/// closure passed to one: no operator, block or branch.
fn place_or_call(expr: &Expr) -> bool {
    match expr {
        Expr::Path(_) | Expr::Closure(_) => true,
        Expr::Tuple(t) => t.elems.iter().all(place_or_call),
        Expr::Lit(l) => !matches!(l.lit, Lit::Bool(_)),
        Expr::Field(e) => place_or_call(&e.base),
        Expr::Reference(e) => place_or_call(&e.expr),
        Expr::Paren(e) => place_or_call(&e.expr),
        Expr::Unary(e) if matches!(e.op, syn::UnOp::Deref(_)) => place_or_call(&e.expr),
        Expr::MethodCall(e) => place_or_call(&e.receiver) && e.args.iter().all(place_or_call),
        Expr::Call(e) => place_or_call(&e.func) && e.args.iter().all(place_or_call),
        _ => false,
    }
}

/// Whether every value `specific` matches, `general` matches too, judged by
/// the patterns' structure. Anything this can't see through counts as covered
/// only when the two are written alike (proposal 0040).
fn covers(general: &syn::Pat, specific: &syn::Pat) -> bool {
    use syn::Pat;
    match (general, specific) {
        (Pat::Wild(_), _) => true,
        (Pat::Paren(g), s) => covers(&g.pat, s),
        (g, Pat::Paren(s)) => covers(g, &s.pat),
        (Pat::Ident(g), _) if g.subpat.is_none() && binds(&g.ident) => true,
        (Pat::Ident(g), s) if g.subpat.is_some() => {
            g.subpat.as_ref().is_some_and(|(_, sub)| covers(sub, s))
        }
        (g, Pat::Ident(s)) if s.subpat.is_some() => {
            s.subpat.as_ref().is_some_and(|(_, sub)| covers(g, sub))
        }
        (g, Pat::Or(s)) => s.cases.iter().all(|case| covers(g, case)),
        (Pat::Or(g), s) => g.cases.iter().any(|case| covers(case, s)),
        (Pat::Reference(g), Pat::Reference(s)) => covers(&g.pat, &s.pat),
        (Pat::Tuple(g), Pat::Tuple(s)) => all_cover(&g.elems, &s.elems),
        (Pat::TupleStruct(g), Pat::TupleStruct(s)) => {
            segments(&g.path) == segments(&s.path) && all_cover(&g.elems, &s.elems)
        }
        (Pat::Struct(g), Pat::Struct(s)) => {
            let wild: Pat = syn::parse_quote!(_);
            segments(&g.path) == segments(&s.path)
                && g.fields.iter().all(|field| {
                    let theirs = s.fields.iter().find(|f| f.member == field.member);
                    covers(&field.pat, theirs.map_or(&wild, |f| &*f.pat))
                })
        }
        (g, s) => g == s,
    }
}

/// A plain lowercase name binds any value; `None` or a `CONST` names one.
fn binds(ident: &syn::Ident) -> bool {
    ident
        .to_string()
        .chars()
        .next()
        .is_some_and(|c| c.is_lowercase() || c == '_')
}

fn all_cover(
    general: &Punctuated<syn::Pat, syn::Token![,]>,
    specific: &Punctuated<syn::Pat, syn::Token![,]>,
) -> bool {
    let rest = |p: &syn::Pat| matches!(p, syn::Pat::Rest(_));
    general.len() == specific.len()
        && !general.iter().chain(specific).any(rest)
        && general.iter().zip(specific).all(|(g, s)| covers(g, s))
}

fn predicate_rule(op: &syn::BinOp) -> Option<&'static Rule> {
    match op {
        syn::BinOp::Eq(_) => Some(&rules::PREDICATE_EQ),
        syn::BinOp::Ne(_) => Some(&rules::PREDICATE_NE),
        syn::BinOp::Lt(_) => Some(&rules::PREDICATE_LT),
        syn::BinOp::Le(_) => Some(&rules::PREDICATE_LE),
        syn::BinOp::Gt(_) => Some(&rules::PREDICATE_GT),
        syn::BinOp::Ge(_) => Some(&rules::PREDICATE_GE),
        syn::BinOp::And(_) => Some(&rules::PREDICATE_AND),
        syn::BinOp::Or(_) => Some(&rules::PREDICATE_OR),
        _ => None,
    }
}

/// A type argument of a type or fn we don't define (proposal 0023).
fn library_type_arg(slot: &Slot) -> bool {
    matches!(slot, Slot::GenericArg { ours: false, .. })
}

/// An argument or field value of a fn, method or struct we don't define
/// (proposal 0023). A method on any receiver but `self` counts as not ours.
fn library_value(slot: &Slot) -> bool {
    matches!(
        slot,
        Slot::CallArg(Callee { ours: false, .. })
            | Slot::MethodArg { on_self: false, .. }
            | Slot::FieldValue { ours: false }
    )
}

/// `drop`, `mem::drop` or `std::mem::drop`.
fn is_drop(path: &syn::Path) -> bool {
    let names: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
    names == ["drop"] || names.ends_with(&["mem".to_string(), "drop".to_string()])
}

fn path_string(path: &syn::Path) -> String {
    path.segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}

fn is_pub_crate(vis: &Visibility) -> bool {
    matches!(vis, Visibility::Restricted(r) if r.in_token.is_none() && r.path.is_ident("crate"))
}

fn field_of_self(expr: &Expr) -> bool {
    match expr {
        Expr::Field(f) => match &*f.base {
            Expr::Path(p) => p.path.is_ident("self"),
            base => field_of_self(base),
        },
        _ => false,
    }
}

/// The last path segment of each name a `#[derive(..)]` lists.
fn derive_names(attr: &Attribute) -> Vec<String> {
    attr.parse_args_with(Punctuated::<syn::Path, syn::Token![,]>::parse_terminated)
        .map(|names| {
            names
                .iter()
                .filter_map(|n| n.segments.last().map(|s| s.ident.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn derives(attr: &Attribute, name: &str) -> bool {
    attr.path().is_ident("derive")
        && attr
            .parse_args_with(Punctuated::<syn::Path, syn::Token![,]>::parse_terminated)
            .is_ok_and(|names| names.iter().any(|n| path_string(n) == name))
}

fn has_message(attr: &Attribute) -> bool {
    let Meta::List(list) = &attr.meta else {
        return false;
    };
    matches!(
        list.tokens.clone().into_iter().next(),
        Some(TokenTree::Literal(lit)) if lit.to_string().starts_with('"')
    )
}

/// The attribute's arguments with all whitespace removed.
fn attr_tokens(attr: &Attribute) -> String {
    match &attr.meta {
        Meta::List(list) => list.tokens.to_string().split_whitespace().collect(),
        _ => String::new(),
    }
}

/// The leading identifier of each comma-separated argument.
fn meta_keys(attr: &Attribute) -> Vec<String> {
    let Meta::List(list) = &attr.meta else {
        return Vec::new();
    };
    let mut keys = Vec::new();
    let mut expect_key = true;
    for tree in list.tokens.clone() {
        match (&tree, expect_key) {
            (TokenTree::Ident(ident), true) => {
                keys.push(ident.to_string());
                expect_key = false;
            }
            (TokenTree::Punct(p), _) if p.as_char() == ',' => expect_key = true,
            _ => expect_key = false,
        }
    }
    keys
}

fn is_cfg(attr: &Attribute) -> bool {
    attr.path().is_ident("cfg") && !is_cfg_test(attr)
}

fn is_cfg_test(attr: &Attribute) -> bool {
    attr.path().is_ident("cfg") && attr_tokens(attr) == "test"
}

/// The predicate of an allowed `cfg`, with whitespace removed.
fn cfg_predicate(attr: &Attribute) -> Option<String> {
    let predicate = attr_tokens(attr);
    CFG_PAIRS
        .iter()
        .any(|(a, b)| predicate == *a || predicate == *b)
        .then_some(predicate)
}

fn complement(predicate: &str) -> String {
    CFG_PAIRS
        .iter()
        .find_map(|(a, b)| match (predicate == *a, predicate == *b) {
            (true, _) => Some(b.to_string()),
            (_, true) => Some(a.to_string()),
            _ => None,
        })
        .unwrap_or_default()
}

fn impl_name(i: &syn::ItemImpl) -> String {
    let ty = facts::type_name(&i.self_ty).unwrap_or_default();
    match &i.trait_ {
        Some((_, path, _)) => format!("{} for {ty}", path_string(path)),
        None => ty,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum CfgKind {
    Fn,
    Impl,
    Use,
}

/// A `cfg` item that needs a counterpart: same kind and name, the other
/// platform's predicate. Every `use` counts as one name.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CfgItem {
    kind: CfgKind,
    name: String,
    predicate: String,
}

impl CfgItem {
    fn new(kind: CfgKind, name: String) -> CfgItem {
        CfgItem {
            kind,
            name,
            predicate: String::new(),
        }
    }
}

/// Every `cfg` fn, impl and use in a file, under each predicate it carries.
struct CfgItems(BTreeSet<CfgItem>);

impl CfgItems {
    fn gather(file: &syn::File) -> CfgItems {
        struct Gather(BTreeSet<CfgItem>);
        impl Gather {
            fn add(&mut self, attrs: &[Attribute], item: CfgItem) {
                for predicate in attrs.iter().filter(|a| is_cfg(a)).filter_map(cfg_predicate) {
                    self.0.insert(CfgItem {
                        predicate,
                        ..item.clone()
                    });
                }
            }
        }
        impl<'ast> Visit<'ast> for Gather {
            fn visit_item(&mut self, item: &'ast Item) {
                match item {
                    Item::Fn(i) => {
                        self.add(&i.attrs, CfgItem::new(CfgKind::Fn, i.sig.ident.to_string()))
                    }
                    Item::Impl(i) => self.add(&i.attrs, CfgItem::new(CfgKind::Impl, impl_name(i))),
                    Item::Use(i) => self.add(&i.attrs, CfgItem::new(CfgKind::Use, String::new())),
                    _ => {}
                }
                visit::visit_item(self, item);
            }
            fn visit_impl_item(&mut self, item: &'ast ImplItem) {
                if let ImplItem::Fn(i) = item {
                    self.add(&i.attrs, CfgItem::new(CfgKind::Fn, i.sig.ident.to_string()));
                }
                visit::visit_impl_item(self, item);
            }
        }
        let mut gather = Gather(BTreeSet::new());
        gather.visit_file(file);
        CfgItems(gather.0)
    }
}

/// The `build` parameter of a private fn `ask<T>` with no bound on `T`, no
/// `where` and two parameters, the last `impl FnOnce(oneshot::Sender<T>) ->
/// Msg` (proposal 0026).
fn ask_build<'s>(vis: &Visibility, sig: &'s syn::Signature) -> Option<&'s Type> {
    let [GenericParam::Type(t)] = sig.generics.params.iter().collect::<Vec<_>>()[..] else {
        return None;
    };
    let plain_t = t.bounds.is_empty() && t.default.is_none() && sig.generics.where_clause.is_none();
    let private = matches!(vis, Visibility::Inherited);
    let (true, true, true, 2) = (sig.ident == "ask", private, plain_t, sig.inputs.len()) else {
        return None;
    };
    let Some(FnArg::Typed(build)) = sig.inputs.last() else {
        return None;
    };
    let Type::ImplTrait(bound) = &*build.ty else {
        return None;
    };
    let [TypeParamBound::Trait(fn_once)] = bound.bounds.iter().collect::<Vec<_>>()[..] else {
        return None;
    };
    let segment = fn_once.path.segments.last()?;
    let PathArguments::Parenthesized(args) = &segment.arguments else {
        return None;
    };
    let [Type::Path(sender)] = args.inputs.iter().collect::<Vec<_>>()[..] else {
        return None;
    };
    let names: Vec<String> = sender
        .path
        .segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect();
    let reply = match &sender.path.segments.last()?.arguments {
        PathArguments::AngleBracketed(a) => match a.args.iter().collect::<Vec<_>>()[..] {
            [GenericArgument::Type(Type::Path(p))] => p.path.is_ident(&t.ident),
            _ => false,
        },
        _ => false,
    };
    let builds_msg =
        matches!(&args.output, ReturnType::Type(_, ty) if matches!(**ty, Type::Path(_)));
    let shaped = fn_once.path.segments.len() == 1
        && segment.ident == "FnOnce"
        && names.ends_with(&["oneshot".to_string(), "Sender".to_string()])
        && reply
        && builds_msg;
    shaped.then_some(&*build.ty)
}

/// The methods of the listed operator traits that a file imports, by name
/// or through a glob of the trait's module (proposal 0038).
fn imported_operator_methods(names: &FileNames, operators: &[Operator]) -> BTreeSet<String> {
    operators
        .iter()
        .filter(|o| {
            names.uses.iter().any(|u| match &u.name {
                Some(_) => o.item.found_in(&u.segments) == u.segments.len().checked_sub(1),
                None => o.item.globbed_by(&u.segments),
            })
        })
        .flat_map(|o| o.methods.iter().cloned())
        .collect()
}

fn segments(path: &syn::Path) -> Vec<String> {
    path.segments.iter().map(|s| s.ident.to_string()).collect()
}

fn param_types(sig: &syn::Signature) -> HashMap<String, String> {
    sig.inputs
        .iter()
        .filter_map(|arg| match arg {
            FnArg::Typed(t) => match (&*t.pat, facts::type_name(&t.ty)) {
                (syn::Pat::Ident(p), Some(ty)) => Some((p.ident.to_string(), ty)),
                _ => None,
            },
            FnArg::Receiver(_) => None,
        })
        .collect()
}

/// Plain names assigned with `=` in a fn body, outside nested items.
fn assigned_names(block: &syn::Block) -> BTreeSet<String> {
    struct Finder(BTreeSet<String>);
    impl<'ast> Visit<'ast> for Finder {
        fn visit_expr_assign(&mut self, e: &'ast syn::ExprAssign) {
            if let Expr::Path(p) = &*e.left {
                self.0.extend(p.path.get_ident().map(ToString::to_string));
            }
            visit::visit_expr_assign(self, e);
        }
        fn visit_item(&mut self, _: &'ast Item) {}
    }
    let mut finder = Finder(BTreeSet::new());
    finder.visit_block(block);
    finder.0
}
