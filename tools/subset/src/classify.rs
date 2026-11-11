//! Walks a parsed file and assigns a construct id to every syntax node that is
//! a construct. Structural nodes (blocks, arms, fields, paths, fn arguments,
//! signatures) carry no id of their own; their modifiers (`async`, `mut`, ...)
//! are reported as ids at the modifier's token.

use proc_macro2::Span;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{
    Attribute, BinOp, Expr, Fields, FnArg, ForeignItem, GenericArgument, GenericParam, ImplItem,
    Item, Lit, Pat, Path, Stmt, TraitItem, Type, TypeParamBound, UnOp, Visibility,
};

use crate::ids;

pub struct Rules<'a> {
    pub derives: &'a [String],
    pub macros: &'a [String],
    pub test_macros: &'a [String],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occurrence {
    pub id: String,
    pub reason: &'static str,
    pub line: usize,
    pub col: usize,
}

pub fn classify(file: &syn::File, rules: &Rules, test_file: bool) -> Vec<Occurrence> {
    let mut classifier = Classifier {
        rules,
        test_depth: usize::from(test_file),
        in_return_type: false,
        in_let_mut: false,
        out: Vec::new(),
    };
    classifier.visit_file(file);
    classifier.out
}

struct Classifier<'r, 'a> {
    rules: &'r Rules<'a>,
    test_depth: usize,
    in_return_type: bool,
    /// Inside the pattern of a `let` already reported as `stmt.let.mut`, so its
    /// `mut` bindings are not reported a second time.
    in_let_mut: bool,
    out: Vec<Occurrence>,
}

impl Classifier<'_, '_> {
    fn emit(&mut self, span: Span, id: &'static str) {
        self.emit_with(span, id.to_string(), ids::reason(id));
    }

    fn emit_with(&mut self, span: Span, id: String, reason: &'static str) {
        let start = span.start();
        self.out.push(Occurrence {
            id,
            reason,
            line: start.line,
            col: start.column + 1,
        });
    }

    fn with_test_scope(&mut self, attrs: &[Attribute], walk: impl FnOnce(&mut Self)) {
        let test = attrs.iter().any(is_test_attr);
        self.test_depth += usize::from(test);
        walk(self);
        self.test_depth -= usize::from(test);
    }

    fn macro_allowed(&self, path: &Path) -> bool {
        let full = path_string(path);
        let last = path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default();
        let listed = |list: &[String]| list.iter().any(|m| *m == full || *m == last);
        listed(self.rules.macros) || (self.test_depth > 0 && listed(self.rules.test_macros))
    }

    fn visit_closure_body(&mut self, closure: &syn::ExprClosure, id: &'static str) {
        self.emit(closure.span(), id);
        if let Some(token) = closure.asyncness {
            self.emit(token.span, "expr.closure.async");
        }
        if let Some(token) = closure.constness {
            self.emit(token.span, "expr.closure.const");
        }
        if let Some(token) = closure.movability {
            self.emit(token.span, "expr.closure.static");
        }
        visit::visit_expr_closure(self, closure);
    }
}

impl<'ast> Visit<'ast> for Classifier<'_, '_> {
    fn visit_item(&mut self, item: &'ast Item) {
        match item {
            Item::Const(i) => self.emit(i.const_token.span, "item.const"),
            Item::Enum(i) => self.emit(i.enum_token.span, "item.enum"),
            Item::ExternCrate(i) => self.emit(i.extern_token.span, "item.extern_crate"),
            Item::Fn(i) => self.emit(i.sig.fn_token.span, "item.fn"),
            Item::ForeignMod(i) => self.emit(i.abi.extern_token.span, "item.foreign_mod"),
            Item::Impl(i) => {
                let id = match i.trait_ {
                    Some(_) => "item.impl.trait",
                    None => "item.impl.inherent",
                };
                self.emit(i.impl_token.span, id);
                if let Some(token) = i.unsafety {
                    self.emit(token.span, "item.impl.unsafe");
                }
            }
            Item::Macro(i) => {
                if i.ident.is_some() && i.mac.path.is_ident("macro_rules") {
                    self.emit(i.mac.path.span(), "item.macro_rules");
                    for attr in &i.attrs {
                        self.visit_attribute(attr);
                    }
                    return;
                }
            }
            Item::Mod(i) => self.emit(i.mod_token.span, "item.mod"),
            Item::Static(i) => {
                let id = match i.mutability {
                    syn::StaticMutability::Mut(_) => "item.static.mut",
                    _ => "item.static",
                };
                self.emit(i.static_token.span, id);
            }
            Item::Struct(i) => {
                let id = match &i.fields {
                    Fields::Named(_) => "item.struct.named",
                    Fields::Unnamed(f) if f.unnamed.len() == 1 => "item.struct.newtype",
                    Fields::Unnamed(_) => "item.struct.tuple",
                    Fields::Unit => "item.struct.unit",
                };
                self.emit(i.struct_token.span, id);
            }
            Item::Trait(i) => {
                self.emit(i.trait_token.span, "item.trait");
                if let Some(token) = i.unsafety {
                    self.emit(token.span, "item.trait.unsafe");
                }
                if let Some(token) = i.auto_token {
                    self.emit(token.span, "item.trait.auto");
                }
            }
            Item::TraitAlias(i) => self.emit(i.trait_token.span, "item.trait_alias"),
            Item::Type(i) => self.emit(i.type_token.span, "item.type"),
            Item::Union(i) => self.emit(i.union_token.span, "item.union"),
            Item::Use(i) => self.emit(i.use_token.span, "item.use"),
            _ => self.emit(item.span(), "unknown.item"),
        }
        self.with_test_scope(item_attrs(item), |this| visit::visit_item(this, item));
    }

    fn visit_impl_item(&mut self, item: &'ast ImplItem) {
        match item {
            ImplItem::Const(i) => self.emit(i.const_token.span, "item.const"),
            ImplItem::Fn(i) => self.emit(i.sig.fn_token.span, "item.fn"),
            ImplItem::Type(i) => self.emit(i.type_token.span, "item.impl.type"),
            ImplItem::Macro(_) => {}
            _ => self.emit(item.span(), "unknown.impl_item"),
        }
        let attrs: &[Attribute] = match item {
            ImplItem::Fn(i) => &i.attrs,
            _ => &[],
        };
        self.with_test_scope(attrs, |this| visit::visit_impl_item(this, item));
    }

    fn visit_trait_item(&mut self, item: &'ast TraitItem) {
        match item {
            TraitItem::Const(i) => self.emit(i.const_token.span, "item.trait.const"),
            TraitItem::Fn(i) => self.emit(i.sig.fn_token.span, "item.trait.fn"),
            TraitItem::Type(i) => self.emit(i.type_token.span, "item.trait.type"),
            TraitItem::Macro(_) => {}
            _ => self.emit(item.span(), "unknown.trait_item"),
        }
        visit::visit_trait_item(self, item);
    }

    fn visit_foreign_item(&mut self, item: &'ast ForeignItem) {
        match item {
            ForeignItem::Fn(i) => self.emit(i.sig.fn_token.span, "item.foreign.fn"),
            ForeignItem::Static(i) => self.emit(i.static_token.span, "item.foreign.static"),
            ForeignItem::Type(i) => self.emit(i.type_token.span, "item.foreign.type"),
            ForeignItem::Macro(_) => {}
            _ => self.emit(item.span(), "unknown.foreign_item"),
        }
        visit::visit_foreign_item(self, item);
    }

    fn visit_variant(&mut self, v: &'ast syn::Variant) {
        if let Some((eq, _)) = &v.discriminant {
            self.emit(eq.span, "item.enum.discriminant");
        }
        visit::visit_variant(self, v);
    }

    fn visit_signature(&mut self, sig: &'ast syn::Signature) {
        if let Some(token) = sig.constness {
            self.emit(token.span, "item.fn.const");
        }
        if let Some(token) = sig.asyncness {
            self.emit(token.span, "item.fn.async");
        }
        if let Some(token) = sig.unsafety {
            self.emit(token.span, "item.fn.unsafe");
        }
        if let Some(abi) = &sig.abi {
            self.emit(abi.extern_token.span, "item.fn.extern");
        }
        if let Some(v) = &sig.variadic {
            self.emit(v.dots.spans[0], "item.fn.variadic");
        }
        self.visit_generics(&sig.generics);
        for arg in &sig.inputs {
            self.visit_fn_arg(arg);
        }
        let outer = std::mem::replace(&mut self.in_return_type, true);
        self.visit_return_type(&sig.output);
        self.in_return_type = outer;
    }

    fn visit_fn_arg(&mut self, arg: &'ast FnArg) {
        match arg {
            FnArg::Receiver(r) => {
                for attr in &r.attrs {
                    self.visit_attribute(attr);
                }
                if let Some(token) = r.mutability {
                    self.emit(token.span, "item.fn.receiver.mut");
                }
                if let Some((_, Some(lifetime))) = &r.reference {
                    self.emit(lifetime.span(), "type.lifetime");
                }
                if r.colon_token.is_some() {
                    self.emit(r.self_token.span, "item.fn.receiver.typed");
                    self.visit_type(&r.ty);
                }
            }
            FnArg::Typed(t) => visit::visit_pat_type(self, t),
        }
    }

    fn visit_visibility(&mut self, vis: &'ast Visibility) {
        match vis {
            Visibility::Public(token) => self.emit(token.span, "vis.pub"),
            Visibility::Restricted(r) => self.emit(r.pub_token.span, "vis.restricted"),
            Visibility::Inherited => {}
        }
    }

    fn visit_use_glob(&mut self, glob: &'ast syn::UseGlob) {
        self.emit(glob.star_token.span, "item.use.glob");
    }

    fn visit_use_rename(&mut self, rename: &'ast syn::UseRename) {
        self.emit(rename.as_token.span, "item.use.rename");
    }

    fn visit_expr(&mut self, expr: &'ast Expr) {
        match expr {
            Expr::Array(e) => self.emit(e.span(), "expr.array"),
            Expr::Assign(e) => self.emit(e.eq_token.span, "expr.assign"),
            Expr::Async(e) => self.emit(e.async_token.span, "expr.async"),
            Expr::Await(e) => self.emit(e.await_token.span, "expr.await"),
            Expr::Binary(e) => self.emit(e.op.span(), binary_id(&e.op)),
            Expr::Block(e) => {
                let id = match e.label {
                    Some(_) => "expr.block.labeled",
                    None => "expr.block",
                };
                self.emit(e.span(), id);
            }
            Expr::Break(e) => self.emit(e.break_token.span, "expr.break"),
            Expr::Call(e) => self.emit(e.span(), "expr.call"),
            Expr::Cast(e) => self.emit(e.as_token.span, "expr.cast"),
            Expr::Closure(e) => return self.visit_closure_body(e, "expr.closure"),
            Expr::Const(e) => self.emit(e.const_token.span, "expr.const_block"),
            Expr::Continue(e) => self.emit(e.continue_token.span, "expr.continue"),
            Expr::Field(e) => self.emit(e.dot_token.span, "expr.field"),
            Expr::ForLoop(e) => self.emit(e.for_token.span, "expr.for"),
            Expr::Group(e) => self.emit(e.span(), "expr.group"),
            Expr::If(e) => {
                if let Expr::Let(cond) = &*e.cond {
                    self.emit(e.if_token.span, "expr.if_let");
                    for attr in &e.attrs {
                        self.visit_attribute(attr);
                    }
                    self.visit_pat(&cond.pat);
                    self.visit_expr(&cond.expr);
                    self.visit_block(&e.then_branch);
                    if let Some((_, else_branch)) = &e.else_branch {
                        self.visit_expr(else_branch);
                    }
                    return;
                }
                self.emit(e.if_token.span, "expr.if");
            }
            Expr::Index(e) => self.emit(e.bracket_token.span.open(), "expr.index"),
            Expr::Infer(e) => self.emit(e.span(), "expr.infer"),
            Expr::Let(e) => self.emit(e.let_token.span, "expr.let"),
            Expr::Lit(e) => {
                let id = match e.lit {
                    Lit::Bool(_) => "expr.lit.bool",
                    _ => "expr.lit",
                };
                self.emit(e.span(), id);
            }
            Expr::Loop(e) => self.emit(e.loop_token.span, "expr.loop"),
            Expr::Macro(_) => {}
            Expr::Match(e) => self.emit(e.match_token.span, "expr.match"),
            Expr::MethodCall(e) => {
                self.emit(e.method.span(), "expr.method_call");
                for attr in &e.attrs {
                    self.visit_attribute(attr);
                }
                self.visit_expr(&e.receiver);
                if let Some(turbofish) = &e.turbofish {
                    self.visit_angle_bracketed_generic_arguments(turbofish);
                }
                for arg in &e.args {
                    match arg {
                        Expr::Closure(c) => self.visit_closure_body(c, "expr.closure.adapter"),
                        other => self.visit_expr(other),
                    }
                }
                return;
            }
            Expr::Paren(e) => self.emit(e.span(), "expr.paren"),
            Expr::Path(e) => match (&e.qself, interior_mut(&e.path)) {
                (None, Some(name)) => {
                    let id = format!("expr.interior_mut.{name}");
                    self.emit_with(e.span(), id, ids::INTERIOR_MUT_EXPR);
                }
                (Some(_), _) => self.emit(e.span(), "expr.path.qualified"),
                (None, None) => self.emit(e.span(), "expr.path"),
            },
            Expr::Range(e) => self.emit(e.limits.span(), "expr.range"),
            Expr::RawAddr(e) => self.emit(e.span(), "expr.raw_addr"),
            Expr::Reference(e) => {
                let id = match e.mutability {
                    Some(_) => "expr.reference.mut",
                    None => "expr.reference",
                };
                self.emit(e.and_token.span, id);
            }
            Expr::Repeat(e) => self.emit(e.span(), "expr.repeat"),
            Expr::Return(e) => self.emit(e.return_token.span, "expr.return"),
            Expr::Struct(e) => {
                let id = match e.rest {
                    Some(_) => "expr.struct.update",
                    None => "expr.struct",
                };
                self.emit(e.span(), id);
            }
            Expr::Try(e) => self.emit(e.question_token.span, "expr.try"),
            Expr::TryBlock(e) => self.emit(e.try_token.span, "expr.try_block"),
            Expr::Tuple(e) => {
                let id = if e.elems.is_empty() {
                    "expr.unit"
                } else {
                    "expr.tuple"
                };
                self.emit(e.span(), id);
            }
            Expr::Unary(e) => {
                let id = match e.op {
                    UnOp::Deref(_) => "expr.unary.deref",
                    UnOp::Not(_) => "expr.unary.not",
                    UnOp::Neg(_) => "expr.unary.neg",
                    _ => "unknown.expr",
                };
                self.emit(e.op.span(), id);
            }
            Expr::Unsafe(e) => self.emit(e.unsafe_token.span, "expr.unsafe"),
            Expr::While(e) => {
                if let Expr::Let(cond) = &*e.cond {
                    self.emit(e.while_token.span, "expr.while_let");
                    for attr in &e.attrs {
                        self.visit_attribute(attr);
                    }
                    self.visit_pat(&cond.pat);
                    self.visit_expr(&cond.expr);
                    self.visit_block(&e.body);
                    return;
                }
                self.emit(e.while_token.span, "expr.while");
            }
            Expr::Yield(e) => self.emit(e.yield_token.span, "expr.yield"),
            _ => return self.emit(expr.span(), "unknown.expr"),
        }
        visit::visit_expr(self, expr);
    }

    fn visit_arm(&mut self, arm: &'ast syn::Arm) {
        if let Some((token, _)) = &arm.guard {
            self.emit(token.span, "expr.match.guard");
        }
        visit::visit_arm(self, arm);
    }

    fn visit_stmt(&mut self, stmt: &'ast Stmt) {
        match stmt {
            Stmt::Local(local) => {
                let mutable = pattern_has_mut(&local.pat);
                let id = match (&local.init, mutable) {
                    (Some(init), _) if init.diverge.is_some() => "stmt.let.else",
                    (_, true) => "stmt.let.mut",
                    (None, false) => "stmt.let.uninit",
                    (Some(_), false) => "stmt.let",
                };
                self.emit(local.let_token.span, id);
                for attr in &local.attrs {
                    self.visit_attribute(attr);
                }
                let outer = std::mem::replace(&mut self.in_let_mut, id == "stmt.let.mut");
                self.visit_pat(&local.pat);
                self.in_let_mut = outer;
                if let Some(init) = &local.init {
                    self.visit_local_init(init);
                }
            }
            _ => visit::visit_stmt(self, stmt),
        }
    }

    fn visit_pat(&mut self, pat: &'ast Pat) {
        match pat {
            Pat::Const(p) => self.emit(p.const_token.span, "pat.const"),
            Pat::Ident(p) => {
                let mutable = p.mutability.is_some() && !self.in_let_mut;
                let id = match (p.by_ref.is_some(), mutable) {
                    (false, false) => "pat.ident",
                    (false, true) => "pat.ident.mut",
                    (true, false) => "pat.ident.ref",
                    (true, true) => "pat.ident.ref_mut",
                };
                self.emit(p.span(), id);
                if let Some((at, _)) = &p.subpat {
                    self.emit(at.span, "pat.ident.at");
                }
            }
            Pat::Lit(p) => {
                let id = match p.lit {
                    Lit::Bool(_) => "pat.lit.bool",
                    _ => "pat.lit",
                };
                return self.emit(p.span(), id);
            }
            Pat::Macro(_) => {}
            Pat::Or(p) => self.emit(p.span(), "pat.or"),
            Pat::Paren(p) => self.emit(p.span(), "pat.paren"),
            Pat::Path(p) => return self.emit(p.span(), "pat.path"),
            Pat::Range(p) => self.emit(p.limits.span(), "pat.range"),
            Pat::Reference(p) => self.emit(p.and_token.span, "pat.reference"),
            Pat::Rest(p) => self.emit(p.span(), "pat.rest"),
            Pat::Slice(p) => self.emit(p.span(), "pat.slice"),
            Pat::Struct(p) => self.emit(p.span(), "pat.struct"),
            Pat::Tuple(p) => self.emit(p.span(), "pat.tuple"),
            Pat::TupleStruct(p) => self.emit(p.span(), "pat.tuple_struct"),
            Pat::Type(p) => self.emit(p.colon_token.span, "pat.type"),
            Pat::Wild(p) => self.emit(p.span(), "pat.wild"),
            _ => return self.emit(pat.span(), "unknown.pat"),
        }
        visit::visit_pat(self, pat);
    }

    fn visit_type(&mut self, ty: &'ast Type) {
        match ty {
            Type::Array(t) => self.emit(t.span(), "type.array"),
            Type::BareFn(t) => self.emit(t.fn_token.span, "type.fn_ptr"),
            Type::Group(t) => self.emit(t.span(), "type.group"),
            Type::ImplTrait(t) => {
                let id = if self.in_return_type {
                    "type.impl_trait.ret"
                } else {
                    "type.impl_trait.arg"
                };
                self.emit(t.impl_token.span, id);
            }
            Type::Infer(t) => self.emit(t.span(), "type.infer"),
            Type::Macro(_) => {}
            Type::Never(t) => self.emit(t.span(), "type.never"),
            Type::Paren(t) => self.emit(t.span(), "type.paren"),
            Type::Path(t) => match (&t.qself, interior_mut(&t.path)) {
                (None, Some(name)) => {
                    let id = format!("type.interior_mut.{name}");
                    self.emit_with(t.span(), id, ids::INTERIOR_MUT_TYPE);
                }
                _ => {
                    let id = match (&t.qself, t.path.is_ident("bool")) {
                        (Some(_), _) => "type.path.qualified",
                        (None, true) => "type.bool",
                        (None, false) => "type.path",
                    };
                    self.emit(t.span(), id);
                }
            },
            Type::Ptr(t) => self.emit(t.span(), "type.ptr"),
            Type::Reference(t) => {
                let id = match t.mutability {
                    Some(_) => "type.reference.mut",
                    None => "type.reference",
                };
                self.emit(t.and_token.span, id);
                if let Some(lifetime) = &t.lifetime {
                    self.emit(lifetime.span(), "type.lifetime");
                }
            }
            Type::Slice(t) => self.emit(t.span(), "type.slice"),
            Type::TraitObject(t) => self.emit(t.span(), "type.dyn"),
            Type::Tuple(t) => {
                let id = if t.elems.is_empty() {
                    "type.unit"
                } else {
                    "type.tuple"
                };
                self.emit(t.span(), id);
            }
            _ => return self.emit(ty.span(), "unknown.type"),
        }
        visit::visit_type(self, ty);
    }

    fn visit_generic_param(&mut self, param: &'ast GenericParam) {
        let id = match param {
            GenericParam::Type(_) => "generics.type_param",
            GenericParam::Lifetime(_) => "generics.lifetime",
            GenericParam::Const(_) => "generics.const_param",
        };
        self.emit(param.span(), id);
        visit::visit_generic_param(self, param);
    }

    fn visit_where_clause(&mut self, clause: &'ast syn::WhereClause) {
        self.emit(clause.where_token.span, "generics.where");
        visit::visit_where_clause(self, clause);
    }

    fn visit_bound_lifetimes(&mut self, bound: &'ast syn::BoundLifetimes) {
        self.emit(bound.for_token.span, "generics.hrtb");
    }

    fn visit_generic_argument(&mut self, arg: &'ast GenericArgument) {
        match arg {
            GenericArgument::Lifetime(l) => self.emit(l.span(), "type.lifetime"),
            GenericArgument::Type(_) => {}
            GenericArgument::Const(c) => self.emit(c.span(), "generics.const_arg"),
            GenericArgument::AssocType(_)
            | GenericArgument::AssocConst(_)
            | GenericArgument::Constraint(_) => self.emit(arg.span(), "generics.assoc_binding"),
            _ => return self.emit(arg.span(), "unknown.generic_arg"),
        }
        visit::visit_generic_argument(self, arg);
    }

    fn visit_type_param_bound(&mut self, bound: &'ast TypeParamBound) {
        match bound {
            TypeParamBound::Trait(_) => {}
            TypeParamBound::Lifetime(l) => self.emit(l.span(), "type.lifetime"),
            _ => return self.emit(bound.span(), "unknown.bound"),
        }
        visit::visit_type_param_bound(self, bound);
    }

    fn visit_attribute(&mut self, attr: &'ast Attribute) {
        let span = attr.pound_token.span;
        let path = attr.path();
        if path.is_ident("doc") {
            return self.emit(span, "attr.doc");
        }
        if path.is_ident("test") || is_cfg_test(attr) {
            return self.emit(span, "attr.cfg_test");
        }
        if path.is_ident("derive") {
            let names = attr.parse_args_with(Punctuated::<Path, syn::Token![,]>::parse_terminated);
            let Ok(names) = names else {
                return self.emit(span, "unknown.attr");
            };
            let unlisted: Vec<String> = names
                .iter()
                .map(path_string)
                .filter(|name| !self.rules.derives.iter().any(|d| d == name))
                .collect();
            if unlisted.is_empty() {
                return self.emit(span, "attr.derive");
            }
            for name in unlisted {
                self.emit_with(span, format!("attr.derive.{name}"), ids::UNLISTED_DERIVE);
            }
            return;
        }
        let id = format!("attr.{}", path_string(path));
        self.emit_with(span, id, ids::UNLISTED_ATTR);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        let span = mac.path.span();
        if self.macro_allowed(&mac.path) {
            self.emit(span, "macro.allowed");
        } else {
            let id = format!("macro.{}", path_string(&mac.path));
            self.emit_with(span, id, ids::UNLISTED_MACRO);
        }
    }
}

fn item_attrs(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(i) => &i.attrs,
        Item::Enum(i) => &i.attrs,
        Item::ExternCrate(i) => &i.attrs,
        Item::Fn(i) => &i.attrs,
        Item::ForeignMod(i) => &i.attrs,
        Item::Impl(i) => &i.attrs,
        Item::Macro(i) => &i.attrs,
        Item::Mod(i) => &i.attrs,
        Item::Static(i) => &i.attrs,
        Item::Struct(i) => &i.attrs,
        Item::Trait(i) => &i.attrs,
        Item::TraitAlias(i) => &i.attrs,
        Item::Type(i) => &i.attrs,
        Item::Union(i) => &i.attrs,
        Item::Use(i) => &i.attrs,
        _ => &[],
    }
}

fn binary_id(op: &BinOp) -> &'static str {
    match op {
        BinOp::Add(_) => "expr.binary.add",
        BinOp::Sub(_) => "expr.binary.sub",
        BinOp::Mul(_) => "expr.binary.mul",
        BinOp::Div(_) => "expr.binary.div",
        BinOp::Rem(_) => "expr.binary.rem",
        BinOp::And(_) => "expr.binary.and",
        BinOp::Or(_) => "expr.binary.or",
        BinOp::BitXor(_) => "expr.binary.bit_xor",
        BinOp::BitAnd(_) => "expr.binary.bit_and",
        BinOp::BitOr(_) => "expr.binary.bit_or",
        BinOp::Shl(_) => "expr.binary.shl",
        BinOp::Shr(_) => "expr.binary.shr",
        BinOp::Eq(_) => "expr.binary.eq",
        BinOp::Lt(_) => "expr.binary.lt",
        BinOp::Le(_) => "expr.binary.le",
        BinOp::Ne(_) => "expr.binary.ne",
        BinOp::Ge(_) => "expr.binary.ge",
        BinOp::Gt(_) => "expr.binary.gt",
        BinOp::AddAssign(_) => "expr.binary.add_assign",
        BinOp::SubAssign(_) => "expr.binary.sub_assign",
        BinOp::MulAssign(_) => "expr.binary.mul_assign",
        BinOp::DivAssign(_) => "expr.binary.div_assign",
        BinOp::RemAssign(_) => "expr.binary.rem_assign",
        BinOp::BitXorAssign(_) => "expr.binary.bit_xor_assign",
        BinOp::BitAndAssign(_) => "expr.binary.bit_and_assign",
        BinOp::BitOrAssign(_) => "expr.binary.bit_or_assign",
        BinOp::ShlAssign(_) => "expr.binary.shl_assign",
        BinOp::ShrAssign(_) => "expr.binary.shr_assign",
        _ => "unknown.expr",
    }
}

/// `#[test]`, `#[tokio::test]`, or `#[cfg(test)]` exactly.
fn is_test_attr(attr: &Attribute) -> bool {
    let path = attr.path();
    path.is_ident("test") || path_string(path) == "tokio::test" || is_cfg_test(attr)
}

fn is_cfg_test(attr: &Attribute) -> bool {
    attr.path().is_ident("cfg")
        && attr
            .parse_args::<syn::Ident>()
            .is_ok_and(|arg| arg == "test")
}

/// Std cell, lock and atomic types: the first path segment that names one.
const INTERIOR_MUT: &[&str] = &[
    "Cell",
    "RefCell",
    "OnceCell",
    "OnceLock",
    "UnsafeCell",
    "Mutex",
    "RwLock",
];

fn interior_mut(path: &Path) -> Option<String> {
    path.segments
        .iter()
        .map(|s| s.ident.to_string())
        .find(|name| INTERIOR_MUT.contains(&name.as_str()) || name.starts_with("Atomic"))
}

fn path_string(path: &Path) -> String {
    path.segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}

fn pattern_has_mut(pat: &Pat) -> bool {
    struct Finder(bool);
    impl<'ast> Visit<'ast> for Finder {
        fn visit_pat_ident(&mut self, p: &'ast syn::PatIdent) {
            self.0 |= p.mutability.is_some();
            visit::visit_pat_ident(self, p);
        }
    }
    let mut finder = Finder(false);
    finder.visit_pat(pat);
    finder.0
}
