//! What the score needs to know about a file beyond its construct ids: the
//! score-only ids that refine an allowed id, the nodes whose inner
//! constructs fold into them, the typing of fields and signatures, and the
//! functions tokens roll up into.

use std::ops::Range;

use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{Expr, Fields, ImplItem, Item, Pat, ReturnType, Stmt, Type, TypeParamBound};

use crate::facts::Facts;

/// The type patterns and method names the score's refinements look for.
pub struct Typing {
    pub adapters: Vec<String>,
    pub primitives: Vec<Type>,
    pub untyped: Vec<Type>,
    /// Names of std containers that typing sees through.
    pub transparent: Vec<String>,
}

/// Which occurrences a refinement applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum From {
    Id(&'static str),
    /// Any type id but the lifetimes.
    AnyType,
}

impl From {
    pub fn covers(self, id: &str) -> bool {
        match self {
            From::Id(from) => id == from,
            From::AnyType => id.starts_with("type.") && !id.starts_with("type.lifetime"),
        }
    }
}

/// An allowed id that the score names more finely, at the occurrence whose
/// extent is `extent`.
pub struct Refinement {
    pub extent: Range<usize>,
    pub from: From,
    pub to: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeClass {
    Own,
    Primitive,
    Untyped,
    Other,
}

impl TypeClass {
    pub const ALL: [TypeClass; 4] = [
        TypeClass::Own,
        TypeClass::Primitive,
        TypeClass::Untyped,
        TypeClass::Other,
    ];

    pub fn index(self) -> usize {
        match self {
            TypeClass::Own => 0,
            TypeClass::Primitive => 1,
            TypeClass::Untyped => 2,
            TypeClass::Other => 3,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            TypeClass::Own => "own",
            TypeClass::Primitive => "primitive",
            TypeClass::Untyped => "untyped",
            TypeClass::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Field,
    Signature,
}

/// A type node in a field of our own type or a signature of our own fn.
pub struct Typed {
    pub extent: Range<usize>,
    pub class: TypeClass,
    pub place: Place,
}

pub struct FnSpan {
    /// `name`, or `Type::name` for a method.
    pub name: String,
    pub line: usize,
    pub extent: Range<usize>,
}

#[derive(Default)]
pub struct Context {
    pub refinements: Vec<Refinement>,
    /// Nodes whose tokens all cost the node's own weight: the head of a
    /// single `if let` (`if let Pat = expr`), and a primitive or untyped type.
    /// The constructs inside one don't own tokens.
    pub folded: Vec<Range<usize>>,
    /// Fields of a type with a library derive in an external protocol module.
    pub wire_fields: Vec<Range<usize>>,
    pub typed: Vec<Typed>,
    /// Primitives in our own fields and signatures held only by transparent
    /// containers, as `String` in `Option<String>`: they keep their tier
    /// where the boundary would relieve a library type's argument.
    pub kept: Vec<Range<usize>>,
    pub fns: Vec<FnSpan>,
}

/// Derives of the standard library; any other derive is a library's.
pub const STD_DERIVES: &[&str] = &[
    "Clone",
    "Copy",
    "Debug",
    "Default",
    "Eq",
    "Hash",
    "Ord",
    "PartialEq",
    "PartialOrd",
];

/// `external` says whether the file mirrors an external protocol.
pub fn gather(file: &syn::File, facts: &Facts, typing: &Typing, external: bool) -> Context {
    let mut gather = Gather {
        facts,
        typing,
        external,
        context: Context::default(),
        self_ty: None,
        foreign_impl: false,
        place: None,
        through: false,
        item: ItemKind::Other,
    };
    gather.visit_file(file);
    gather.context
}

#[derive(Clone, Copy)]
enum ItemKind {
    Other,
    Newtype,
    Wire,
}

struct Gather<'a> {
    facts: &'a Facts,
    typing: &'a Typing,
    external: bool,
    context: Context,
    self_ty: Option<String>,
    /// Inside an impl of a trait we don't define, whose signatures the
    /// trait fixes.
    foreign_impl: bool,
    place: Option<Place>,
    /// Every type between here and the field or signature type is a
    /// transparent container.
    through: bool,
    item: ItemKind,
}

impl Gather<'_> {
    fn function(&mut self, ident: &syn::Ident, node: proc_macro2::Span) {
        let name = match &self.self_ty {
            Some(ty) => format!("{ty}::{ident}"),
            None => ident.to_string(),
        };
        self.context.fns.push(FnSpan {
            name,
            line: ident.span().start().line,
            extent: node.byte_range(),
        });
    }

    fn refine(&mut self, node: &Type, to: &'static str) {
        self.context.refinements.push(Refinement {
            extent: node.span().byte_range(),
            from: From::AnyType,
            to,
        });
    }

    fn typed(&mut self, node: &Type, class: TypeClass, place: Place) {
        self.context.typed.push(Typed {
            extent: node.span().byte_range(),
            class,
            place,
        });
    }

    fn in_place(&mut self, place: Place, ty: &Type) {
        let outer = (self.place.replace(place), self.through);
        self.through = true;
        self.visit_type(ty);
        (self.place, self.through) = outer;
    }

    /// A std container that typing sees through, such as `Option<T>`.
    fn transparent(&self, ty: &Type) -> bool {
        match ty {
            Type::Path(p) if p.qself.is_none() && !self.facts.is_ours(&p.path) => p
                .path
                .segments
                .last()
                .is_some_and(|s| self.typing.transparent.iter().any(|t| s.ident == t)),
            _ => false,
        }
    }

    /// Visits the types inside `ty`, which holds them transparently or not.
    fn visit_inside(&mut self, ty: &Type, transparent: bool) {
        let outer = self.through;
        self.through = outer && transparent;
        visit::visit_type(self, ty);
        self.through = outer;
    }

    fn class(&self, ty: &Type) -> TypeClass {
        let matches = |patterns: &[Type]| patterns.iter().any(|p| type_matches(p, ty, self.facts));
        match ty {
            _ if matches(&self.typing.untyped) => TypeClass::Untyped,
            _ if matches(&self.typing.primitives) => TypeClass::Primitive,
            Type::Reference(r) if self.class(&r.elem) == TypeClass::Primitive => {
                TypeClass::Primitive
            }
            Type::Path(p) if p.qself.is_none() && self.facts.is_ours(&p.path) => TypeClass::Own,
            _ => TypeClass::Other,
        }
    }
}

impl<'ast> Visit<'ast> for Gather<'_> {
    fn visit_item(&mut self, item: &'ast Item) {
        let outer = (self.self_ty.take(), self.foreign_impl, self.item);
        self.foreign_impl = false;
        self.item = ItemKind::Other;
        match item {
            Item::Fn(i) => self.function(&i.sig.ident, item.span()),
            Item::Impl(i) => {
                self.self_ty =
                    Some(crate::facts::type_name(&i.self_ty).unwrap_or_else(|| "_".to_string()));
                self.foreign_impl = i
                    .trait_
                    .as_ref()
                    .is_some_and(|(_, path, _)| !self.facts.is_ours(path));
            }
            Item::Struct(i) => {
                self.item = match (wire(&i.attrs, self.external), &i.fields) {
                    (true, _) => ItemKind::Wire,
                    (false, Fields::Unnamed(f)) if f.unnamed.len() == 1 => ItemKind::Newtype,
                    _ => ItemKind::Other,
                };
            }
            Item::Enum(i) => {
                self.item = match wire(&i.attrs, self.external) {
                    true => ItemKind::Wire,
                    false => ItemKind::Other,
                };
            }
            _ => {}
        }
        visit::visit_item(self, item);
        (self.self_ty, self.foreign_impl, self.item) = outer;
    }

    fn visit_impl_item(&mut self, item: &'ast ImplItem) {
        if let ImplItem::Fn(i) = item {
            self.function(&i.sig.ident, item.span());
        }
        visit::visit_impl_item(self, item);
    }

    fn visit_field(&mut self, field: &'ast syn::Field) {
        for attr in &field.attrs {
            self.visit_attribute(attr);
        }
        match self.item {
            ItemKind::Wire => {
                self.context.wire_fields.push(field.span().byte_range());
                self.visit_type(&field.ty);
            }
            ItemKind::Newtype => {
                let outer = self.place.take();
                match self.class(&field.ty) {
                    TypeClass::Untyped => self.visit_type(&field.ty),
                    _ => {
                        self.refine(&field.ty, "type.field");
                        self.context.folded.push(field.ty.span().byte_range());
                    }
                }
                self.place = outer;
            }
            ItemKind::Other => self.in_place(Place::Field, &field.ty),
        }
    }

    fn visit_signature(&mut self, sig: &'ast syn::Signature) {
        if self.foreign_impl {
            return visit::visit_signature(self, sig);
        }
        self.visit_generics(&sig.generics);
        for arg in &sig.inputs {
            match arg {
                syn::FnArg::Typed(t) => {
                    self.visit_pat(&t.pat);
                    self.in_place(Place::Signature, &t.ty);
                }
                receiver => self.visit_fn_arg(receiver),
            }
        }
        if let ReturnType::Type(_, ty) = &sig.output {
            self.in_place(Place::Signature, ty);
        }
    }

    fn visit_type(&mut self, ty: &'ast Type) {
        let class = self.class(ty);
        match (self.place, class) {
            (place, TypeClass::Untyped) => {
                self.refine(ty, "type.untyped");
                self.context.folded.push(ty.span().byte_range());
                if let Some(place) = place {
                    self.typed(ty, class, place);
                }
            }
            (Some(place), TypeClass::Primitive) => {
                self.refine(ty, "type.primitive");
                self.context.folded.push(ty.span().byte_range());
                self.typed(ty, class, place);
                if self.through {
                    self.context.kept.push(ty.span().byte_range());
                }
            }
            (Some(place), _) => {
                if place == Place::Field {
                    self.refine(ty, "type.field");
                }
                let transparent = self.transparent(ty);
                if !transparent {
                    self.typed(ty, class, place);
                }
                self.visit_inside(ty, transparent);
            }
            (None, _) => visit::visit_type(self, ty),
        }
    }

    fn visit_expr(&mut self, expr: &'ast Expr) {
        let outer = self.place.take();
        match expr {
            Expr::MethodCall(e) if self.typing.adapters.iter().any(|a| e.method == a) => {
                self.context.refinements.push(Refinement {
                    extent: expr.span().byte_range(),
                    from: From::Id("expr.method_call"),
                    to: "expr.method_call.adapter",
                });
            }
            Expr::If(e) if e.else_branch.is_none() => {
                if let Expr::Let(cond) = &*e.cond {
                    let start = e.if_token.span.byte_range().start;
                    let end = cond.expr.span().byte_range().end;
                    self.context.folded.push(start..end);
                }
            }
            _ => {}
        }
        visit::visit_expr(self, expr);
        self.place = outer;
    }

    fn visit_stmt(&mut self, stmt: &'ast Stmt) {
        if let Stmt::Local(local) = stmt
            && matches!(local.pat, Pat::Wild(_))
        {
            self.context.refinements.push(Refinement {
                extent: local.span().byte_range(),
                from: From::Id("stmt.let"),
                to: "stmt.let.discard",
            });
        }
        visit::visit_stmt(self, stmt);
    }
}

/// A type with a library derive, in a file that mirrors an external protocol.
fn wire(attrs: &[syn::Attribute], external: bool) -> bool {
    external
        && attrs.iter().any(|attr| {
            attr.path().is_ident("derive")
                && attr
                    .parse_args_with(
                        syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated,
                    )
                    .is_ok_and(|names| {
                        names.iter().any(|n| {
                            n.segments.last().is_some_and(|s| {
                                !STD_DERIVES.contains(&s.ident.to_string().as_str())
                            })
                        })
                    })
        })
}

/// Whether `ty` has the shape of `pattern`. `_` matches any type. Paths match
/// on their last segment and on their generic arguments where the pattern
/// gives them. A pattern's leading segments must match where the type spells
/// them too; a bare name matches only if it isn't one we define.
pub fn type_matches(pattern: &Type, ty: &Type, facts: &Facts) -> bool {
    match (pattern, ty) {
        (Type::Infer(_), _) => true,
        (Type::Paren(p), _) => type_matches(&p.elem, ty, facts),
        (_, Type::Paren(t)) => type_matches(pattern, &t.elem, facts),
        (Type::Reference(p), Type::Reference(t)) => {
            p.mutability.is_some() == t.mutability.is_some()
                && type_matches(&p.elem, &t.elem, facts)
        }
        (Type::Path(p), Type::Path(t)) if p.qself.is_none() && t.qself.is_none() => {
            path_matches(&p.path, &t.path, facts)
        }
        (Type::TraitObject(p), Type::TraitObject(t)) => p.bounds.iter().all(|pb| {
            t.bounds.iter().any(|tb| match (pb, tb) {
                (TypeParamBound::Trait(pb), TypeParamBound::Trait(tb)) => {
                    path_matches(&pb.path, &tb.path, facts)
                }
                _ => false,
            })
        }),
        _ => false,
    }
}

fn path_matches(pattern: &syn::Path, path: &syn::Path, facts: &Facts) -> bool {
    let names =
        |p: &syn::Path| -> Vec<String> { p.segments.iter().map(|s| s.ident.to_string()).collect() };
    let (want, have) = (names(pattern), names(path));
    let shared = want.len().min(have.len());
    let tails_agree = want[want.len() - shared..] == have[have.len() - shared..];
    let bare_and_ours = have.len() < want.len() && facts.is_ours(path);
    let (Some(p_last), Some(t_last)) = (pattern.segments.last(), path.segments.last()) else {
        return false;
    };
    let args_agree = match (&p_last.arguments, &t_last.arguments) {
        (syn::PathArguments::None, _) => true,
        (syn::PathArguments::AngleBracketed(p), syn::PathArguments::AngleBracketed(t)) => {
            p.args.len() == t.args.len()
                && p.args.iter().zip(&t.args).all(|pair| match pair {
                    (syn::GenericArgument::Type(p), syn::GenericArgument::Type(t)) => {
                        type_matches(p, t, facts)
                    }
                    _ => false,
                })
        }
        _ => false,
    };
    shared > 0 && tails_agree && !bare_and_ours && args_agree
}
