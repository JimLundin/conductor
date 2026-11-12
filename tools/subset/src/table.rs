//! The `const` tables of a file and the row types they hold (proposal 0042):
//! which structs may have `&'static str` fields, and which string fields a
//! scan of each table could look up by equality.

use std::collections::{BTreeSet, HashMap, HashSet};

use syn::visit::{self, Visit};
use syn::{Expr, Fields, Type};

pub struct Tables {
    /// Each `const` table's name, with the string fields of its rows: the
    /// field names of a row struct, or the indexes of a tuple row.
    keys: HashMap<String, BTreeSet<String>>,
    /// Structs that are the row type of a `const` table and are built only
    /// in `const` initializers.
    rows: HashSet<String>,
}

impl Tables {
    pub fn gather(file: &syn::File) -> Tables {
        let mut walk = Walk::default();
        walk.visit_file(file);
        let rows = walk
            .tables
            .values()
            .filter_map(row_name)
            .filter(|name| walk.structs.contains_key(name) && !walk.built_outside.contains(name))
            .collect();
        let keys = walk
            .tables
            .iter()
            .map(|(table, row)| (table.clone(), string_fields(row, &walk.structs)))
            .filter(|(_, fields)| !fields.is_empty())
            .collect();
        Tables { keys, rows }
    }

    pub fn is_row(&self, name: &str) -> bool {
        self.rows.contains(name)
    }

    /// The string fields of the table that `expr` scans: a `const` table's
    /// name, optionally behind `&`, parentheses and `iter`, `into_iter`,
    /// `copied`, `cloned` or `rev`.
    pub fn scanned(&self, expr: &Expr) -> Option<&BTreeSet<String>> {
        match expr {
            Expr::MethodCall(e)
                if ["iter", "into_iter", "copied", "cloned", "rev"]
                    .contains(&e.method.to_string().as_str()) =>
            {
                self.scanned(&e.receiver)
            }
            Expr::Reference(e) => self.scanned(&e.expr),
            Expr::Paren(e) => self.scanned(&e.expr),
            Expr::Path(e) => {
                let name = e.path.segments.last()?.ident.to_string();
                self.keys.get(&name)
            }
            _ => None,
        }
    }
}

/// Whether `ty` is `&str` or `&'static str`.
pub fn is_str_ref(ty: &Type) -> bool {
    matches!(ty, Type::Reference(r) if r.mutability.is_none() && matches!(&*r.elem, Type::Path(p) if p.qself.is_none() && p.path.is_ident("str")))
}

/// Whether `ty` is `&'static str`.
pub fn is_static_str(ty: &Type) -> bool {
    is_str_ref(ty)
        && matches!(ty, Type::Reference(r) if r.lifetime.as_ref().is_some_and(|l| l.ident == "static"))
}

#[derive(Default)]
struct Walk {
    /// Each `const` whose type is a slice or array, with its element type.
    tables: HashMap<String, Type>,
    /// Each struct with named fields, with the names of its `&str` fields.
    structs: HashMap<String, BTreeSet<String>>,
    /// Structs built by a struct literal outside a `const` initializer.
    built_outside: HashSet<String>,
    in_const: usize,
}

impl<'ast> Visit<'ast> for Walk {
    fn visit_item_const(&mut self, i: &'ast syn::ItemConst) {
        if let Some(row) = element(&i.ty) {
            self.tables.insert(i.ident.to_string(), row.clone());
        }
        self.in_const += 1;
        visit::visit_item_const(self, i);
        self.in_const -= 1;
    }

    fn visit_impl_item_const(&mut self, i: &'ast syn::ImplItemConst) {
        if let Some(row) = element(&i.ty) {
            self.tables.insert(i.ident.to_string(), row.clone());
        }
        self.in_const += 1;
        visit::visit_impl_item_const(self, i);
        self.in_const -= 1;
    }

    fn visit_item_struct(&mut self, i: &'ast syn::ItemStruct) {
        if let Fields::Named(fields) = &i.fields {
            let strings = fields
                .named
                .iter()
                .filter(|f| is_str_ref(&f.ty))
                .filter_map(|f| f.ident.as_ref().map(ToString::to_string))
                .collect();
            self.structs.insert(i.ident.to_string(), strings);
        }
        visit::visit_item_struct(self, i);
    }

    fn visit_expr_struct(&mut self, e: &'ast syn::ExprStruct) {
        if let (0, Some(last)) = (self.in_const, e.path.segments.last()) {
            self.built_outside.insert(last.ident.to_string());
        }
        visit::visit_expr_struct(self, e);
    }
}

/// The element type of `&[T]`, `[T; N]` or `&[T; N]`.
fn element(ty: &Type) -> Option<&Type> {
    match ty {
        Type::Reference(r) => match &*r.elem {
            Type::Slice(s) => Some(&s.elem),
            Type::Array(a) => Some(&a.elem),
            _ => None,
        },
        Type::Array(a) => Some(&a.elem),
        _ => None,
    }
}

fn row_name(row: &Type) -> Option<String> {
    match row {
        Type::Path(p) if p.qself.is_none() => p.path.get_ident().map(ToString::to_string),
        _ => None,
    }
}

fn string_fields(row: &Type, structs: &HashMap<String, BTreeSet<String>>) -> BTreeSet<String> {
    match row {
        Type::Tuple(t) => t
            .elems
            .iter()
            .enumerate()
            .filter(|(_, ty)| is_str_ref(ty))
            .map(|(i, _)| i.to_string())
            .collect(),
        _ => row_name(row)
            .and_then(|name| structs.get(&name).cloned())
            .unwrap_or_default(),
    }
}
