//! Which names a lambda body closes over.
//!
//! A lambda used to capture its whole defining scope
//! ([`Environment`](crate::env::Environment)), which is a reference cycle the
//! moment the lambda is stored back into that scope — the overwhelmingly
//! common shape, `local f = fn() … end`:
//!
//! ```text
//! Environment ──vars──▶ Value::Function ──closure──▶ Environment
//! ```
//!
//! Nothing in that loop is weak, so the scope and everything bound in it
//! leaked on *every execution*. Capturing the handful of names the body
//! actually mentions breaks the cycle structurally — see
//! [`Environment::capture_flat`](crate::env::Environment::capture_flat).
//!
//! ## Why "mentions" and not "free variables"
//!
//! This deliberately does **not** subtract names the body binds itself. A
//! precise free-variable analysis would have to model every binding form, and
//! the two ways of being wrong are not symmetric:
//!
//! * Capturing a name the body did not need is harmless. The capture walk
//!   simply finds nothing to promote, or promotes a binding that is then
//!   shadowed at call time — lookup consults the call scope before the
//!   captured one, so shadowing still wins.
//! * *Missing* a name the body did need is a runtime `undefined name` on code
//!   that used to work.
//!
//! So the walker collects every identifier that appears anywhere in the body,
//! including inside nested lambdas, and lets the capture walk sort out which
//! of them actually resolve to an enclosing scope.
//!
//! ## The bail-out
//!
//! [`Stmt::Decl`] inside a lambda body is a construct this analysis does not
//! model — a nested `fn`, `class`, `enum`, or `import`. None of them are legal
//! there today (`saule_semantic`'s resolver rejects a nested named function
//! outright), but rather than assume that stays true, hitting one sets
//! `opaque` and yields [`None`]. The caller then falls back to capturing the
//! whole scope, which is exactly the old behaviour: correct, and leaky only
//! for a construct that cannot currently occur.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use saule_ast::{CallArg, Expr, LambdaBody, MatchBody, Pattern, Spanned, Stmt, TableEntry};

use crate::fxhash::FxHashMap as HashMap;

/// Names a lambda body mentions, or `None` when the body contained something
/// [the walker does not model](self#the-bail-out).
pub(crate) type Captures = Option<Rc<[Rc<str>]>>;

thread_local! {
    /// Memo keyed on the body's `Arc` address.
    ///
    /// A lambda written inside a loop is evaluated once per iteration but
    /// shares one `Arc` body (that is why [`LambdaBody`] holds one), so the
    /// walk happens once per lambda in the source rather than once per
    /// closure created.
    ///
    /// The body is stored alongside the result purely to keep that `Arc`
    /// alive: a freed allocation could otherwise be reused at the same
    /// address by an unrelated lambda and collide with this key.
    static CACHE: RefCell<HashMap<usize, (LambdaBody, Captures)>> =
        RefCell::new(HashMap::default());
}

/// The names `body` mentions, computed once per distinct lambda body.
pub(crate) fn captured_names(body: &LambdaBody) -> Captures {
    let key = body_key(body);
    if let Some(hit) = CACHE.with(|c| c.borrow().get(&key).map(|(_, v)| v.clone())) {
        return hit;
    }

    let mut w = Walker::default();
    match body {
        LambdaBody::Expr(e) => w.expr(e),
        LambdaBody::Block(stmts) => w.block(stmts),
    }
    let out = if w.opaque {
        None
    } else {
        Some(Rc::from(w.names.into_boxed_slice()))
    };

    CACHE.with(|c| {
        c.borrow_mut().insert(key, (body.clone(), out.clone()));
    });
    out
}

/// Identity of a lambda body: the address its `Arc` points at.
///
/// Empty blocks may share one well-known dangling address across every empty
/// `Arc<[T]>`, so two distinct empty bodies can collide. Both capture nothing,
/// which makes the collision unobservable.
fn body_key(body: &LambdaBody) -> usize {
    match body {
        LambdaBody::Expr(e) => Arc::as_ptr(e) as *const u8 as usize,
        LambdaBody::Block(stmts) => stmts.as_ptr() as *const u8 as usize,
    }
}

#[derive(Default)]
struct Walker {
    /// Insertion-ordered and deduped. A capture set is a handful of names, so
    /// a linear scan beats hashing.
    names: Vec<Rc<str>>,
    opaque: bool,
}

impl Walker {
    fn add(&mut self, name: &str) {
        if !self.names.iter().any(|n| &**n == name) {
            self.names.push(Rc::from(name));
        }
    }

    fn block(&mut self, stmts: &[Spanned<Stmt>]) {
        for s in stmts {
            self.stmt(s);
        }
    }

    fn exprs(&mut self, es: &[Spanned<Expr>]) {
        for e in es {
            self.expr(e);
        }
    }

    fn args(&mut self, args: &[CallArg]) {
        for a in args {
            match a {
                CallArg::Positional(e) => self.expr(e),
                CallArg::Named { value, .. } => self.expr(value),
            }
        }
    }

    fn stmt(&mut self, s: &Spanned<Stmt>) {
        match &s.value {
            Stmt::Local { value, .. } => {
                if let Some(v) = value {
                    self.expr(v);
                }
            }
            Stmt::LocalMulti { values, .. } => self.exprs(values),
            // The target matters as much as the value: `count = count + 1`
            // inside a lambda has to reach the enclosing `count`.
            Stmt::Assign { target, value } => {
                self.expr(target);
                self.expr(value);
            }
            Stmt::AssignMulti { targets, values } => {
                self.exprs(targets);
                self.exprs(values);
            }
            Stmt::CompoundAssign { target, value, .. } => {
                self.expr(target);
                self.expr(value);
            }
            Stmt::Expr(e) => self.expr(e),
            Stmt::If {
                cond,
                then_block,
                elseifs,
                else_block,
            } => {
                self.expr(cond);
                self.block(then_block);
                for (c, b) in elseifs {
                    self.expr(c);
                    self.block(b);
                }
                if let Some(b) = else_block {
                    self.block(b);
                }
            }
            Stmt::While { cond, body } => {
                self.expr(cond);
                self.block(body);
            }
            Stmt::Repeat { body, cond } => {
                self.block(body);
                self.expr(cond);
            }
            Stmt::ForNumeric {
                from,
                to,
                step,
                body,
                ..
            } => {
                self.expr(from);
                self.expr(to);
                if let Some(st) = step {
                    self.expr(st);
                }
                self.block(body);
            }
            Stmt::ForIn { iter, body, .. } => {
                self.expr(iter);
                self.block(body);
            }
            Stmt::Return(es) => self.exprs(es),
            Stmt::Throw(e) => self.expr(e),
            Stmt::Try {
                body, catch_body, ..
            } => {
                self.block(body);
                self.block(catch_body);
            }
            Stmt::Break | Stmt::Continue | Stmt::Error => {}
            // See the module docs: not modelled, so give up and let the
            // caller keep the old whole-scope capture.
            Stmt::Decl(_) => self.opaque = true,
        }
    }

    fn expr(&mut self, e: &Spanned<Expr>) {
        match &e.value {
            Expr::Int(_)
            | Expr::Float(_)
            | Expr::Bool(_)
            | Expr::Str(_)
            | Expr::Nil
            | Expr::Error => {}

            Expr::Ident(name) => self.add(name),
            Expr::Self_ => self.add("self"),

            Expr::Unary { rhs, .. } => self.expr(rhs),
            Expr::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            // `name` is a field, not a binding — only the receiver can name
            // something in an enclosing scope.
            Expr::Member { obj, .. } | Expr::SafeMember { obj, .. } => self.expr(obj),
            Expr::Index { obj, index } => {
                self.expr(obj);
                self.expr(index);
            }
            Expr::Call { callee, args } => {
                self.expr(callee);
                self.args(args);
            }
            Expr::ForceUnwrap(v) => self.expr(v),
            Expr::Cast { value, .. } => self.expr(value),
            Expr::Table(entries) => {
                for entry in entries {
                    match entry {
                        TableEntry::Positional(v) => self.expr(v),
                        TableEntry::Field { key, value } => {
                            self.expr(key);
                            self.expr(value);
                        }
                    }
                }
            }
            // A nested lambda's own captures resolve through *this* lambda's
            // scope, so they have to be captured here too.
            Expr::Lambda { body, .. } => match body {
                LambdaBody::Expr(inner) => self.expr(inner),
                LambdaBody::Block(stmts) => self.block(stmts),
            },
            Expr::Match { scrutinee, arms } => {
                self.expr(scrutinee);
                for arm in arms {
                    self.pattern(&arm.pattern);
                    if let Some(g) = &arm.guard {
                        self.expr(g);
                    }
                    match &arm.body {
                        MatchBody::Expr(v) => self.expr(v),
                        MatchBody::Block(stmts) => self.block(stmts),
                    }
                }
            }
            Expr::Pipe { source, stages } => {
                self.expr(source);
                for stage in stages {
                    // Each stage names a free function, resolved as a
                    // plain identifier.
                    self.add(&stage.name);
                    self.args(&stage.args);
                }
            }
        }
    }

    /// Patterns bind rather than read, with one exception: a variant pattern
    /// names its enum, and that name resolves like any other.
    fn pattern(&mut self, p: &Spanned<Pattern>) {
        match &p.value {
            Pattern::Variant {
                enum_name, fields, ..
            } => {
                self.add(enum_name);
                for f in fields {
                    self.pattern(f);
                }
            }
            Pattern::Tuple(items) => {
                for f in items {
                    self.pattern(f);
                }
            }
            Pattern::Wildcard
            | Pattern::Bind(_)
            | Pattern::Nil
            | Pattern::Int(_)
            | Pattern::Float(_)
            | Pattern::Bool(_)
            | Pattern::Str(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use saule_ast::Module;

    /// The capture set of the first `local f = fn … end` in `src`, sorted so
    /// assertions do not depend on walk order.
    fn captures_of(src: &str) -> Option<Vec<String>> {
        let tokens = saule_lexer::Lexer::new(src)
            .tokenize()
            .expect("test source should lex");
        let module = saule_parser::parse(tokens).expect("test source should parse");
        let body = first_lambda(&module).expect("test source should contain a lambda");
        captured_names(&body).map(|names| {
            let mut v: Vec<String> = names.iter().map(|n| n.to_string()).collect();
            v.sort();
            v
        })
    }

    fn first_lambda(module: &Module) -> Option<LambdaBody> {
        module.stmts.iter().find_map(|s| match &s.value {
            Stmt::Local {
                value: Some(value), ..
            } => match &value.value {
                Expr::Lambda { body, .. } => Some(body.clone()),
                _ => None,
            },
            _ => None,
        })
    }

    #[test]
    fn collects_identifiers_the_body_reads() {
        let got = captures_of("local a = 1\nlocal b = 2\nlocal f = fn() return a + b end");
        assert_eq!(got, Some(vec!["a".into(), "b".into()]));
    }

    #[test]
    fn collects_assignment_targets() {
        // `n = n + 1` has to reach the enclosing `n`, so the target counts
        // as a mention just as much as the value does.
        let got = captures_of("local n = 0\nlocal f = fn() n = n + 1 end");
        assert_eq!(got, Some(vec!["n".into()]));
    }

    #[test]
    fn collects_through_nested_lambdas() {
        let got = captures_of("local outer = 1\nlocal f = fn() return fn() return outer end end");
        assert_eq!(got, Some(vec!["outer".into()]));
    }

    #[test]
    fn collects_self() {
        let got = captures_of("local f = fn() return self.label end");
        assert_eq!(got, Some(vec!["self".into()]));
    }

    /// A field name is not a binding — only the receiver can name something
    /// in an enclosing scope.
    #[test]
    fn member_names_are_not_captures() {
        let got = captures_of("local obj = 1\nlocal f = fn() return obj.field end");
        assert_eq!(got, Some(vec!["obj".into()]));
    }

    /// Over-approximating is deliberate: `x` is rebound by the parameter but
    /// still collected, and shadowing is resolved at call time instead.
    #[test]
    fn shadowed_names_are_still_collected() {
        let got = captures_of("local x = 1\nlocal f = fn(x: integer) return x end");
        assert_eq!(got, Some(vec!["x".into()]));
    }

    #[test]
    fn literals_capture_nothing() {
        let got = captures_of("local f = fn() return 1 + 2 end");
        assert_eq!(got, Some(vec![]));
    }

    #[test]
    fn caches_per_body() {
        let src = "local a = 1\nlocal f = fn() return a end";
        assert_eq!(captures_of(src), captures_of(src));
    }
}
