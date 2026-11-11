//! Names gathered across every checked file before any file is checked, so
//! a rule can tell our own types and functions from a library's. They are
//! names, not resolved paths: a library type that shares a name with one of
//! ours counts as ours.

use std::collections::BTreeSet;
use std::path::{Component, Path};

use syn::visit::{self, Visit};
use syn::{Expr, FnArg, GenericArgument, Pat, PathArguments, Type};

use crate::actor;

#[derive(Default)]
pub struct Facts {
    /// Our crates' names: the directory above each file's `src`, `tests` or
    /// `examples`.
    pub crates: BTreeSet<String>,
    /// Module names.
    pub mods: BTreeSet<String>,
    /// Struct, enum, union, trait and type alias names.
    pub types: BTreeSet<String>,
    /// Free fn names.
    pub fns: BTreeSet<String>,
    /// Types named as the `E` of some `Result<T, E>`.
    pub error_types: BTreeSet<String>,
    /// Actor loop states: the `Self` of a `mut self` method whose body is an
    /// actor loop, and the type a loop's `let mut` state is built from.
    pub loop_states: BTreeSet<String>,
    /// Newtype structs that derive `Ord` (proposal 0024).
    pub ord_newtypes: BTreeSet<String>,
}

impl Facts {
    pub fn gather<'a>(files: impl IntoIterator<Item = (&'a Path, &'a syn::File)>) -> Facts {
        let mut gather = Gather {
            facts: Facts::default(),
            self_ty: None,
        };
        for (path, file) in files {
            gather.facts.crates.extend(crate_name(path));
            gather.visit_file(file);
        }
        gather.facts
    }

    /// Whether a type or value path names something of ours: a path from
    /// `crate`, `self`, `super`, one of our crates or one of our modules, a
    /// single name we define, or an associated item of a type we define.
    pub fn is_ours(&self, path: &syn::Path) -> bool {
        let names: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        match names.as_slice() {
            [] => false,
            [only] => self.types.contains(only) || self.fns.contains(only),
            [first, ..]
                if ["crate", "self", "super", "Self"].contains(&first.as_str())
                    || self.crates.contains(first)
                    || self.mods.contains(first) =>
            {
                true
            }
            [first, _] => self.types.contains(first),
            _ => false,
        }
    }
}

struct Gather {
    facts: Facts,
    self_ty: Option<String>,
}

impl Gather {
    fn loop_fn(&mut self, inputs: Option<&FnArg>, block: &syn::Block) {
        let Some(actor) = actor::actor_loop(block) else {
            return;
        };
        let owned_mut_self = matches!(inputs, Some(FnArg::Receiver(r)) if r.reference.is_none() && r.mutability.is_some());
        if let (true, Some(ty)) = (owned_mut_self, &self.self_ty) {
            self.facts.loop_states.insert(ty.clone());
        }
        if let Some(name) = actor.state.and_then(state_type) {
            self.facts.loop_states.insert(name);
        }
    }
}

impl<'ast> Visit<'ast> for Gather {
    fn visit_item_struct(&mut self, i: &'ast syn::ItemStruct) {
        self.facts.types.insert(i.ident.to_string());
        if is_newtype(&i.fields) && i.attrs.iter().any(|a| derives(a, "Ord")) {
            self.facts.ord_newtypes.insert(i.ident.to_string());
        }
        visit::visit_item_struct(self, i);
    }

    fn visit_item_enum(&mut self, i: &'ast syn::ItemEnum) {
        self.facts.types.insert(i.ident.to_string());
        visit::visit_item_enum(self, i);
    }

    fn visit_item_union(&mut self, i: &'ast syn::ItemUnion) {
        self.facts.types.insert(i.ident.to_string());
        visit::visit_item_union(self, i);
    }

    fn visit_item_trait(&mut self, i: &'ast syn::ItemTrait) {
        self.facts.types.insert(i.ident.to_string());
        visit::visit_item_trait(self, i);
    }

    fn visit_item_type(&mut self, i: &'ast syn::ItemType) {
        self.facts.types.insert(i.ident.to_string());
        visit::visit_item_type(self, i);
    }

    fn visit_item_mod(&mut self, i: &'ast syn::ItemMod) {
        self.facts.mods.insert(i.ident.to_string());
        visit::visit_item_mod(self, i);
    }

    fn visit_item_fn(&mut self, i: &'ast syn::ItemFn) {
        self.facts.fns.insert(i.sig.ident.to_string());
        self.loop_fn(i.sig.inputs.first(), &i.block);
        visit::visit_item_fn(self, i);
    }

    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        let outer = std::mem::replace(&mut self.self_ty, type_name(&i.self_ty));
        visit::visit_item_impl(self, i);
        self.self_ty = outer;
    }

    fn visit_impl_item_fn(&mut self, i: &'ast syn::ImplItemFn) {
        self.loop_fn(i.sig.inputs.first(), &i.block);
        visit::visit_impl_item_fn(self, i);
    }

    fn visit_path_segment(&mut self, segment: &'ast syn::PathSegment) {
        if let (true, PathArguments::AngleBracketed(args)) =
            (segment.ident == "Result", &segment.arguments)
        {
            let error = args
                .args
                .iter()
                .filter_map(|arg| match arg {
                    GenericArgument::Type(ty) => Some(ty),
                    _ => None,
                })
                .nth(1)
                .and_then(type_name);
            self.facts.error_types.extend(error);
        }
        visit::visit_path_segment(self, segment);
    }
}

pub fn is_newtype(fields: &syn::Fields) -> bool {
    matches!(fields, syn::Fields::Unnamed(f) if f.unnamed.len() == 1)
}

/// Whether a `#[derive(..)]` lists `name`, matched on the last path segment.
pub fn derives(attr: &syn::Attribute, name: &str) -> bool {
    attr.path().is_ident("derive")
        && attr
            .parse_args_with(
                syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated,
            )
            .is_ok_and(|names| {
                names
                    .iter()
                    .any(|n| n.segments.last().is_some_and(|s| s.ident == name))
            })
}

/// The last path segment of a type, through references.
pub fn type_name(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(t) => t.path.segments.last().map(|s| s.ident.to_string()),
        Type::Reference(t) => type_name(&t.elem),
        Type::Paren(t) => type_name(&t.elem),
        _ => None,
    }
}

/// The type a `let mut` loop state is built from: its ascribed type, the
/// struct it constructs, or the type of the associated fn it calls.
fn state_type(local: &syn::Local) -> Option<String> {
    if let Pat::Type(t) = &local.pat {
        return type_name(&t.ty);
    }
    let init = &local.init.as_ref()?.expr;
    match actor::strip(init) {
        Expr::Struct(e) => e.path.segments.last().map(|s| s.ident.to_string()),
        Expr::Call(call) => match &*call.func {
            Expr::Path(p) => {
                let segments: Vec<_> = p.path.segments.iter().collect();
                segments
                    .len()
                    .checked_sub(2)
                    .map(|i| segments[i].ident.to_string())
            }
            _ => None,
        },
        _ => None,
    }
}

fn crate_name(path: &Path) -> Option<String> {
    let parts: Vec<&str> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect();
    parts
        .windows(2)
        .find(|pair| ["src", "tests", "examples"].contains(&pair[1]))
        .map(|pair| pair[0].to_string())
}
