//! The `const` tables of a file and the row types they hold (proposal 0042):
//! which structs may have `&'static str` fields, and which predicates on a
//! table are keyed lookups that should be a `match`.

use std::collections::{BTreeSet, HashMap, HashSet};

use syn::visit::{self, Visit};
use syn::{Expr, Fields, Pat, Type};

pub struct Tables {
    /// Each `const` table's name, with what a keyed lookup compares in it.
    keys: HashMap<String, Keys>,
    /// The row structs: each is the element type of a `const` table and is
    /// built only as an element of a table's literal.
    rows: HashSet<*const syn::ItemStruct>,
}

/// What a keyed lookup compares in a table's rows.
#[derive(Clone)]
pub enum Keys {
    /// The `&str` fields of a row struct, by name, or of a tuple row, by
    /// index.
    Fields(BTreeSet<String>),
    /// The rows themselves, in a table of `&str`.
    Element,
}

/// A table that an iterator chain reads, and whether `enumerate` pairs
/// each row with its index.
pub struct Scan<'a> {
    keys: &'a Keys,
    enumerated: bool,
}

/// Methods between a table and its predicate that pass rows through.
const PASS_THROUGH: &[&str] = &[
    "iter",
    "into_iter",
    "copied",
    "cloned",
    "rev",
    "skip",
    "take",
    "step_by",
    "filter",
    "skip_while",
    "take_while",
    "by_ref",
];

impl Tables {
    pub fn gather(file: &syn::File) -> Tables {
        let mut walk = Walk::default();
        walk.visit_file(file);
        let excluded = |s: &StructInfo| {
            walk.outside.contains(&s.key)
                || walk
                    .outside_names
                    .contains(s.key.last().map_or("", String::as_str))
        };
        let rows = walk
            .structs
            .iter()
            .filter(|s| walk.tables.iter().any(|t| t.row.as_ref() == Some(&s.key)))
            .filter(|s| !excluded(s))
            .map(|s| s.ptr)
            .collect();
        let keys = walk
            .tables
            .iter()
            .filter_map(|t| t.keys(&walk.structs).map(|keys| (t.name.clone(), keys)))
            .collect();
        Tables { keys, rows }
    }

    pub fn is_row(&self, item: &syn::ItemStruct) -> bool {
        self.rows.contains(&std::ptr::from_ref(item))
    }

    /// The table that `expr` reads: a `const` table by name, or a name
    /// bound to one in `bindings`, behind `&`, parentheses and the methods
    /// in `PASS_THROUGH` or `enumerate`.
    pub fn scanned(&self, expr: &Expr, bindings: &HashMap<String, Expr>) -> Option<Scan<'_>> {
        match expr {
            Expr::MethodCall(e) if e.method == "enumerate" => {
                let inner = self.scanned(&e.receiver, bindings)?;
                Some(Scan {
                    enumerated: true,
                    ..inner
                })
            }
            Expr::MethodCall(e) if PASS_THROUGH.contains(&e.method.to_string().as_str()) => {
                self.scanned(&e.receiver, bindings)
            }
            Expr::Reference(e) => self.scanned(&e.expr, bindings),
            Expr::Paren(e) => self.scanned(&e.expr, bindings),
            Expr::Path(e) => {
                let bound = e
                    .path
                    .get_ident()
                    .and_then(|i| bindings.get(&i.to_string()));
                match bound {
                    Some(init) => self.scanned(init, &HashMap::new()),
                    None => {
                        let name = e.path.segments.last()?.ident.to_string();
                        self.keys.get(&name).map(|keys| Scan {
                            keys,
                            enumerated: false,
                        })
                    }
                }
            }
            _ => None,
        }
    }

    /// The names a block binds with `let` to a table or a chain over one.
    pub fn bindings(&self, block: &syn::Block) -> HashMap<String, Expr> {
        struct Finder<'t> {
            tables: &'t Tables,
            found: HashMap<String, Expr>,
        }
        impl<'ast> Visit<'ast> for Finder<'_> {
            fn visit_local(&mut self, local: &'ast syn::Local) {
                if let (Pat::Ident(p), Some(init)) = (&local.pat, &local.init)
                    && self.tables.scanned(&init.expr, &HashMap::new()).is_some()
                {
                    self.found.insert(p.ident.to_string(), (*init.expr).clone());
                }
                visit::visit_local(self, local);
            }
            fn visit_item(&mut self, _: &'ast syn::Item) {}
        }
        let mut finder = Finder {
            tables: self,
            found: HashMap::new(),
        };
        finder.visit_block(block);
        finder.found
    }
}

/// Whether a predicate closure over `scan`'s rows tests a string key for
/// equality with a value that isn't a literal, in any spelling: `==`,
/// `!(.. != ..)`, `.eq`, `PartialEq::eq`, `cmp(..).is_eq()`, inside `&&`,
/// `||` or a block.
pub fn keyed(closure: &syn::ExprClosure, scan: &Scan) -> bool {
    let Some(pat) = closure.inputs.first() else {
        return false;
    };
    let row = match (scan.enumerated, unref(pat)) {
        (false, pat) => pat,
        (true, Pat::Tuple(t)) if t.elems.len() == 2 => &t.elems[1],
        (true, _) => return false,
    };
    let binding = Binding::of(row, scan.keys);
    binding.compares(&closure.body, false, scan.keys)
}

/// How a predicate closure binds a row.
enum Binding {
    /// The whole row, by name.
    Whole(String),
    /// The row's string keys, by the names a pattern binds them to.
    Parts(BTreeSet<String>),
}

impl Binding {
    fn of(pat: &Pat, keys: &Keys) -> Binding {
        let fields = match keys {
            Keys::Fields(fields) => fields.clone(),
            Keys::Element => BTreeSet::new(),
        };
        match unref(pat) {
            Pat::Ident(p) => Binding::Whole(p.ident.to_string()),
            Pat::Tuple(t) => Binding::Parts(
                t.elems
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| fields.contains(&i.to_string()))
                    .filter_map(|(_, p)| ident(p))
                    .collect(),
            ),
            Pat::Struct(s) => Binding::Parts(
                s.fields
                    .iter()
                    .filter(|f| match &f.member {
                        syn::Member::Named(name) => fields.contains(&name.to_string()),
                        syn::Member::Unnamed(_) => false,
                    })
                    .filter_map(|f| ident(&f.pat))
                    .collect(),
            ),
            _ => Binding::Parts(BTreeSet::new()),
        }
    }

    /// Whether `expr`, read as a predicate (negated when `negated`), holds
    /// an equality between a key and a value that isn't a literal.
    fn compares(&self, expr: &Expr, negated: bool, keys: &Keys) -> bool {
        let equal = |a: &Expr, b: &Expr| {
            (self.key(a, keys) && !literal(b)) || (self.key(b, keys) && !literal(a))
        };
        let eq = match negated {
            false => "eq",
            true => "ne",
        };
        match expr {
            Expr::Paren(e) => self.compares(&e.expr, negated, keys),
            Expr::Block(b) => match b.block.stmts.as_slice() {
                [syn::Stmt::Expr(tail, None)] => self.compares(tail, negated, keys),
                _ => false,
            },
            Expr::Unary(e) if matches!(e.op, syn::UnOp::Not(_)) => {
                self.compares(&e.expr, !negated, keys)
            }
            Expr::Binary(e) => match e.op {
                syn::BinOp::And(_) | syn::BinOp::Or(_) => {
                    self.compares(&e.left, negated, keys) || self.compares(&e.right, negated, keys)
                }
                syn::BinOp::Eq(_) if !negated => equal(&e.left, &e.right),
                syn::BinOp::Ne(_) if negated => equal(&e.left, &e.right),
                _ => false,
            },
            Expr::MethodCall(e) if e.method == eq && e.args.len() == 1 => {
                equal(&e.receiver, &e.args[0])
            }
            Expr::MethodCall(e) if e.method == format!("is_{eq}") => match &*e.receiver {
                Expr::MethodCall(c)
                    if (c.method == "cmp" || c.method == "partial_cmp") && c.args.len() == 1 =>
                {
                    equal(&c.receiver, &c.args[0])
                }
                _ => false,
            },
            Expr::Call(e) if e.args.len() == 2 => {
                let named = matches!(&*e.func, Expr::Path(p) if p.path.segments.last().is_some_and(|s| s.ident == eq));
                named && equal(&e.args[0], &e.args[1])
            }
            _ => false,
        }
    }

    fn key(&self, expr: &Expr, keys: &Keys) -> bool {
        match (self, expr, keys) {
            (_, Expr::Paren(e), _) => self.key(&e.expr, keys),
            (_, Expr::Reference(e), _) => self.key(&e.expr, keys),
            (_, Expr::Unary(e), _) if matches!(e.op, syn::UnOp::Deref(_)) => {
                self.key(&e.expr, keys)
            }
            (Binding::Whole(name), Expr::Path(p), Keys::Element) => p.path.is_ident(name),
            (Binding::Whole(name), Expr::Field(f), Keys::Fields(fields)) => {
                let field = match &f.member {
                    syn::Member::Named(i) => i.to_string(),
                    syn::Member::Unnamed(i) => i.index.to_string(),
                };
                fields.contains(&field) && self.key_base(&f.base, name)
            }
            (Binding::Parts(names), Expr::Path(p), _) => p
                .path
                .get_ident()
                .is_some_and(|i| names.contains(&i.to_string())),
            _ => false,
        }
    }

    fn key_base(&self, base: &Expr, name: &str) -> bool {
        match base {
            Expr::Paren(e) => self.key_base(&e.expr, name),
            Expr::Unary(e) if matches!(e.op, syn::UnOp::Deref(_)) => self.key_base(&e.expr, name),
            Expr::Path(p) => p.path.is_ident(name),
            _ => false,
        }
    }
}

fn unref(pat: &Pat) -> &Pat {
    match pat {
        Pat::Reference(p) => unref(&p.pat),
        Pat::Type(p) => unref(&p.pat),
        _ => pat,
    }
}

fn ident(pat: &Pat) -> Option<String> {
    match unref(pat) {
        Pat::Ident(p) => Some(p.ident.to_string()),
        _ => None,
    }
}

fn literal(expr: &Expr) -> bool {
    match expr {
        Expr::Paren(e) => literal(&e.expr),
        Expr::Reference(e) => literal(&e.expr),
        Expr::Lit(_) => true,
        _ => false,
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

/// The element type of `&[T]`, `[T; N]` or `&[T; N]`.
pub fn element(ty: &Type) -> Option<&Type> {
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

/// Whether a tuple type holds a `&str`, through nested tuples.
pub fn holds_str(ty: &Type) -> bool {
    match ty {
        Type::Tuple(t) => t.elems.iter().any(holds_str),
        _ => is_str_ref(ty),
    }
}

/// A struct with named fields, by its module path within the file.
struct StructInfo {
    key: Vec<String>,
    ptr: *const syn::ItemStruct,
    /// The names of its `&str` fields.
    strings: BTreeSet<String>,
}

/// A `const` whose type is a slice or array.
struct Table {
    name: String,
    element: Type,
    /// The element type resolved to a struct's module path, when it names
    /// one in this file's module tree.
    row: Option<Vec<String>>,
}

impl Table {
    fn keys(&self, structs: &[StructInfo]) -> Option<Keys> {
        let keys = match (&self.element, &self.row) {
            (Type::Tuple(t), _) => Keys::Fields(
                t.elems
                    .iter()
                    .enumerate()
                    .filter(|(_, ty)| is_str_ref(ty))
                    .map(|(i, _)| i.to_string())
                    .collect(),
            ),
            (ty, _) if is_str_ref(ty) => Keys::Element,
            (_, Some(row)) => Keys::Fields(structs.iter().find(|s| &s.key == row)?.strings.clone()),
            _ => return None,
        };
        match &keys {
            Keys::Fields(fields) if fields.is_empty() => None,
            _ => Some(keys),
        }
    }
}

#[derive(Default)]
struct Walk {
    module: Vec<String>,
    structs: Vec<StructInfo>,
    tables: Vec<Table>,
    /// Struct literals that are elements of a table's literal of their own
    /// type.
    table_builds: HashSet<*const syn::ExprStruct>,
    /// Structs, by module path, built outside a table's literal or held
    /// by a `const` of their own type.
    outside: HashSet<Vec<String>>,
    /// The same, by bare name, where the path didn't resolve in this file.
    outside_names: HashSet<String>,
}

impl Walk {
    /// A path as a module path within the file: `self::` and `super::`
    /// resolved, a bare name in the current module. `crate::` paths and
    /// paths with generic arguments don't resolve.
    fn resolve(&self, path: &syn::Path) -> Option<Vec<String>> {
        if path.leading_colon.is_some()
            || path
                .segments
                .iter()
                .any(|s| !matches!(s.arguments, syn::PathArguments::None))
        {
            return None;
        }
        let mut out = self.module.clone();
        let mut segments = path.segments.iter().peekable();
        while let Some(s) = segments.peek() {
            match s.ident.to_string().as_str() {
                "self" => {}
                "super" => {
                    out.pop()?;
                }
                "crate" => return None,
                _ => break,
            }
            segments.next();
        }
        out.extend(segments.map(|s| s.ident.to_string()));
        Some(out)
    }

    fn mark_outside(&mut self, path: &syn::Path) {
        match self.resolve(path) {
            Some(key) if self.structs.iter().any(|s| s.key == key) => {
                self.outside.insert(key);
            }
            _ => {
                if let Some(last) = path.segments.last() {
                    self.outside_names.insert(last.ident.to_string());
                }
            }
        }
    }

    fn visit_const(&mut self, name: &syn::Ident, ty: &Type, init: &Expr) {
        match (element(ty), ty) {
            (Some(row), _) => {
                let key = match row {
                    Type::Path(p) if p.qself.is_none() => self.resolve(&p.path),
                    _ => None,
                };
                if let Some(key) = &key {
                    for elem in literal_rows(init) {
                        if let Expr::Struct(s) = elem
                            && self.resolve(&s.path).as_ref() == Some(key)
                        {
                            self.table_builds.insert(std::ptr::from_ref(s));
                        }
                    }
                }
                self.tables.push(Table {
                    name: name.to_string(),
                    element: row.clone(),
                    row: key,
                });
            }
            (None, Type::Path(p)) if p.qself.is_none() => self.mark_outside(&p.path),
            _ => {}
        }
    }
}

/// The elements of an array literal, behind `&` and parentheses.
fn literal_rows(expr: &Expr) -> Vec<&Expr> {
    match expr {
        Expr::Reference(e) => literal_rows(&e.expr),
        Expr::Paren(e) => literal_rows(&e.expr),
        Expr::Array(a) => a.elems.iter().collect(),
        _ => Vec::new(),
    }
}

impl<'ast> Visit<'ast> for Walk {
    fn visit_file(&mut self, file: &'ast syn::File) {
        // Structs first, so a const or literal earlier in the file resolves.
        let mut structs = StructWalk {
            module: Vec::new(),
            out: Vec::new(),
        };
        structs.visit_file(file);
        self.structs = structs.out;
        visit::visit_file(self, file);
    }

    fn visit_item_mod(&mut self, i: &'ast syn::ItemMod) {
        self.module.push(i.ident.to_string());
        visit::visit_item_mod(self, i);
        self.module.pop();
    }

    fn visit_item_const(&mut self, i: &'ast syn::ItemConst) {
        self.visit_const(&i.ident, &i.ty, &i.expr);
        visit::visit_item_const(self, i);
    }

    fn visit_impl_item_const(&mut self, i: &'ast syn::ImplItemConst) {
        self.visit_const(&i.ident, &i.ty, &i.expr);
        visit::visit_impl_item_const(self, i);
    }

    fn visit_expr_struct(&mut self, e: &'ast syn::ExprStruct) {
        if !self.table_builds.contains(&std::ptr::from_ref(e)) {
            self.mark_outside(&e.path);
        }
        visit::visit_expr_struct(self, e);
    }
}

struct StructWalk {
    module: Vec<String>,
    out: Vec<StructInfo>,
}

impl<'ast> Visit<'ast> for StructWalk {
    fn visit_item_mod(&mut self, i: &'ast syn::ItemMod) {
        self.module.push(i.ident.to_string());
        visit::visit_item_mod(self, i);
        self.module.pop();
    }

    fn visit_item_struct(&mut self, i: &'ast syn::ItemStruct) {
        if let Fields::Named(fields) = &i.fields {
            let mut key = self.module.clone();
            key.push(i.ident.to_string());
            self.out.push(StructInfo {
                key,
                ptr: std::ptr::from_ref(i),
                strings: fields
                    .named
                    .iter()
                    .filter(|f| is_str_ref(&f.ty))
                    .filter_map(|f| f.ident.as_ref().map(ToString::to_string))
                    .collect(),
            });
        }
        visit::visit_item_struct(self, i);
    }
}
