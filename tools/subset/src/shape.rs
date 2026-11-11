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
use crate::rules::{self, Rule};

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
}

impl Lists<'_> {
    fn predicate(&self, method: &str) -> bool {
        self.predicate_adapters.iter().any(|m| m == method)
    }

    fn takes_closure(&self, method: &str) -> bool {
        self.predicate(method) || self.closure_methods.iter().any(|m| m == method)
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
    let mut checker = Checker {
        facts,
        kind,
        lists,
        exprs: HashMap::new(),
        types: HashMap::new(),
        cfg_hosts: HashMap::new(),
        cfg_items: CfgItems::gather(file),
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
}

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
        });
        if let Some(FnArg::Receiver(r)) = sig.inputs.first() {
            self.check_receiver(r);
        }
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
        for field in &i.fields {
            if clap {
                self.set_type(&field.ty, Slot::ClapField);
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
        self.set_type(&i.ty, Slot::ConstType);
        self.set_expr(&i.expr, Slot::ConstInit);
        visit::visit_item_const(self, i);
    }

    fn visit_impl_item_const(&mut self, i: &'ast syn::ImplItemConst) {
        self.set_type(&i.ty, Slot::ConstType);
        self.set_expr(&i.expr, Slot::ConstInit);
        visit::visit_impl_item_const(self, i);
    }

    fn visit_item_use(&mut self, i: &'ast syn::ItemUse) {
        self.check_use_tree(&i.tree, &mut Vec::new());
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
                let drop = matches!(&*e.func, Expr::Path(p) if is_drop(&p.path));
                let named =
                    matches!(e.args.first(), Some(Expr::Path(p)) if p.path.get_ident().is_some());
                if drop && !named {
                    self.flag(e.func.span(), &rules::DROP_DISCARD);
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
            Expr::While(_) | Expr::ForLoop(_) => {
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

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        if !STD_MACROS.contains(&path_string(&mac.path).as_str()) {
            self.mark(mac.span(), Fixed::Always);
        }
        if mac.path.is_ident("matches") && !self.predicate_macros.contains(&std::ptr::from_ref(mac))
        {
            self.flag(mac.path.span(), &rules::PREDICATE_MATCHES);
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
