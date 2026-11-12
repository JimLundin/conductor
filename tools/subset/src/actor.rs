//! The actor loop of proposals 0007 and 0008: where a `loop` may stand and
//! what one turn of it holds. `facts` uses it to find loop-state types, and
//! `shape` to check each `loop`.

use syn::visit::{self, Visit};
use syn::{Block, Expr, Local, Pat, Stmt};

/// A fn body that is one `loop`, after at most one `let mut` state binding.
pub struct ActorLoop<'a> {
    pub state: Option<&'a Local>,
    pub body: &'a syn::ExprLoop,
}

pub fn actor_loop(block: &Block) -> Option<ActorLoop<'_>> {
    let (last, before) = block.stmts.split_last()?;
    let Stmt::Expr(Expr::Loop(body), _) = last else {
        return None;
    };
    let state = match before {
        [] => None,
        [Stmt::Local(local)] if !mut_bindings(&local.pat).is_empty() => Some(local),
        _ => return None,
    };
    Some(ActorLoop { state, body })
}

/// One turn takes the next input, then handles it with at most one call or
/// `match`.
pub fn turn_is_simple(body: &Block) -> bool {
    match body.stmts.as_slice() {
        [take] => takes_input(take),
        [take, handle] => takes_input(take) && handles(handle),
        _ => false,
    }
}

fn takes_input(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Macro(m) => is_select(&m.mac),
        Stmt::Local(local) => local.init.as_ref().is_some_and(|init| {
            init.diverge.is_none() && (awaits(&init.expr) || is_select_expr(&init.expr))
        }),
        Stmt::Expr(Expr::Macro(m), _) => is_select(&m.mac),
        Stmt::Expr(Expr::Match(m), _) => {
            awaits(&m.expr)
                || m.arms
                    .iter()
                    .all(|arm| is_select_expr(&arm.body) || awaits(&arm.body))
        }
        Stmt::Expr(expr, _) => awaits(expr),
        Stmt::Item(_) => false,
    }
}

fn handles(stmt: &Stmt) -> bool {
    let Stmt::Expr(expr, _) = stmt else {
        return false;
    };
    match strip(expr) {
        Expr::Call(_) | Expr::MethodCall(_) | Expr::Match(_) => true,
        Expr::Assign(a) => matches!(strip(&a.right), Expr::Call(_) | Expr::MethodCall(_)),
        _ => false,
    }
}

/// The expression under any trailing `?` and `.await`.
pub fn strip(expr: &Expr) -> &Expr {
    match expr {
        Expr::Try(e) => strip(&e.expr),
        Expr::Await(e) => strip(&e.base),
        other => other,
    }
}

fn is_select(mac: &syn::Macro) -> bool {
    mac.path
        .segments
        .last()
        .is_some_and(|s| s.ident == "select")
}

fn is_select_expr(expr: &Expr) -> bool {
    matches!(expr, Expr::Macro(m) if is_select(&m.mac))
}

/// Whether `expr` awaits, outside any closure or async block it contains.
fn awaits(expr: &Expr) -> bool {
    struct Finder(bool);
    impl<'ast> Visit<'ast> for Finder {
        fn visit_expr_await(&mut self, _: &'ast syn::ExprAwait) {
            self.0 = true;
        }
        fn visit_expr_closure(&mut self, _: &'ast syn::ExprClosure) {}
        fn visit_expr_async(&mut self, _: &'ast syn::ExprAsync) {}
        fn visit_item(&mut self, _: &'ast syn::Item) {}
    }
    let mut finder = Finder(false);
    finder.visit_expr(expr);
    finder.0
}

/// The names a pattern binds with `mut`.
pub fn mut_bindings(pat: &Pat) -> Vec<String> {
    struct Finder(Vec<String>);
    impl<'ast> Visit<'ast> for Finder {
        fn visit_pat_ident(&mut self, p: &'ast syn::PatIdent) {
            if p.mutability.is_some() {
                self.0.push(p.ident.to_string());
            }
            visit::visit_pat_ident(self, p);
        }
    }
    let mut finder = Finder(Vec::new());
    finder.visit_pat(pat);
    finder.0
}
