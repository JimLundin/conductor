//! What one file's own text says about the names it uses: the types it
//! defines and where each name it imports comes from. The facts in `facts`
//! are names across every file, so two crates' `Command` look alike there;
//! a file's imports tell them apart.

use std::collections::BTreeSet;

use proc_macro2::Span;
use syn::UseTree;
use syn::visit::{self, Visit};

use crate::facts::Facts;

/// One path a `use` item brings in, as `std::ops::Add`, with the name it
/// binds (its last segment or its `as` name). A glob has no name.
pub struct UsePath {
    pub segments: Vec<String>,
    pub name: Option<String>,
    /// The span of the path's last segment, or of the `*`.
    pub span: Span,
}

/// Every path a `use` tree brings in.
pub fn use_paths(tree: &UseTree) -> Vec<UsePath> {
    fn walk(tree: &UseTree, prefix: &mut Vec<String>, out: &mut Vec<UsePath>) {
        let leaf = |prefix: &[String], ident: &syn::Ident, name: Option<String>| UsePath {
            segments: prefix
                .iter()
                .cloned()
                .chain(std::iter::once(ident.to_string()))
                .collect(),
            name,
            span: ident.span(),
        };
        match tree {
            UseTree::Path(p) => {
                prefix.push(p.ident.to_string());
                walk(&p.tree, prefix, out);
                prefix.pop();
            }
            UseTree::Name(n) => out.push(leaf(prefix, &n.ident, Some(n.ident.to_string()))),
            UseTree::Rename(r) => out.push(leaf(prefix, &r.ident, Some(r.rename.to_string()))),
            UseTree::Glob(g) => out.push(UsePath {
                segments: prefix.clone(),
                name: None,
                span: g.star_token.span,
            }),
            UseTree::Group(g) => {
                for tree in &g.items {
                    walk(tree, prefix, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(tree, &mut Vec::new(), &mut out);
    out
}

/// The names one file defines and imports.
#[derive(Default)]
pub struct FileNames {
    /// Structs, enums, unions and type aliases defined in the file.
    pub defined: BTreeSet<String>,
    /// Names imported from a path of ours.
    pub ours: BTreeSet<String>,
    /// Names imported from a path we don't define.
    pub foreign: BTreeSet<String>,
    /// The file has a glob import of a path of ours, as `use super::*`.
    pub our_glob: bool,
    /// Every path the file's `use` items bring in.
    pub uses: Vec<UsePath>,
}

impl FileNames {
    pub fn gather(file: &syn::File, facts: &Facts) -> FileNames {
        struct Gather(FileNames);
        impl<'ast> Visit<'ast> for Gather {
            fn visit_item_struct(&mut self, i: &'ast syn::ItemStruct) {
                self.0.defined.insert(i.ident.to_string());
                visit::visit_item_struct(self, i);
            }
            fn visit_item_enum(&mut self, i: &'ast syn::ItemEnum) {
                self.0.defined.insert(i.ident.to_string());
                visit::visit_item_enum(self, i);
            }
            fn visit_item_union(&mut self, i: &'ast syn::ItemUnion) {
                self.0.defined.insert(i.ident.to_string());
                visit::visit_item_union(self, i);
            }
            fn visit_item_type(&mut self, i: &'ast syn::ItemType) {
                self.0.defined.insert(i.ident.to_string());
                visit::visit_item_type(self, i);
            }
            fn visit_item_use(&mut self, i: &'ast syn::ItemUse) {
                self.0.uses.extend(use_paths(&i.tree));
            }
        }
        let mut gather = Gather(FileNames::default());
        gather.visit_file(file);
        let mut names = gather.0;
        for path in &names.uses {
            let ours = path
                .segments
                .first()
                .is_some_and(|first| rooted_in_ours(first, facts));
            match (&path.name, ours) {
                (Some(name), true) => names.ours.insert(name.clone()),
                (Some(name), false) => names.foreign.insert(name.clone()),
                (None, ours) => {
                    names.our_glob |= ours;
                    false
                }
            };
        }
        names
    }

    /// Whether a path that starts with `first` starts from something of
    /// ours in this file.
    pub fn rooted_in_ours(&self, first: &str, facts: &Facts) -> bool {
        !self.foreign.contains(first) && (self.ours.contains(first) || rooted_in_ours(first, facts))
    }

    /// Whether a bare type name in this file is a type of ours: imported
    /// from a path of ours, defined here, or reachable through a glob of
    /// ours. A name imported from a library never is.
    pub fn bare_type_is_ours(&self, name: &str, facts: &Facts) -> bool {
        facts.types.contains(name)
            && !self.foreign.contains(name)
            && (self.ours.contains(name) || self.defined.contains(name) || self.our_glob)
    }
}

/// `crate`, `self`, `super`, one of our crates or one of our modules.
fn rooted_in_ours(first: &str, facts: &Facts) -> bool {
    ["crate", "self", "super"].contains(&first)
        || facts.crates.contains(first)
        || facts.mods.contains(first)
}
