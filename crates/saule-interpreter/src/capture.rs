//! Which names a lambda body closes over — looked up, no longer guessed.
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
//! leaked on *every execution*. Capturing only the names the body actually
//! needs breaks the cycle structurally — see
//! [`Environment::capture_flat`](crate::env::Environment::capture_flat).
//!
//! ## What changed
//!
//! This module used to *compute* the capture set itself, by collecting every
//! identifier appearing anywhere in the body. That was deliberately an
//! over-approximation, and it had a hole it documented honestly: a nested
//! `Stmt::Decl` was a construct it did not model, so it gave up and returned
//! `None`, and the caller fell back to capturing the whole scope. Correct,
//! and leaky.
//!
//! The analysis now lives in `saule-semantic`, which is the only pass that
//! already carries the scope stack needed to answer the question *exactly*
//! (`saule_semantic::analyze_with_bindings`, `VM_DESIGN.md` §21.1 item 0.6).
//! What is left here is a registry: the pipeline hands over the answer for a
//! module, and lambda evaluation looks it up.
//!
//! ## Why the key is the body's address and not its `NodeId`
//!
//! Node ids are per module and each module numbers from zero, so a `NodeId`
//! alone would collide across modules — and a program with imports evaluates
//! lambdas from several. A `LambdaBody`'s `Arc` address is globally unique
//! while the body is alive, and the entry holds the body to keep it that way.
//!
//! It is also the key the old memo used, for the same reason it is a good
//! one: a lambda written inside a loop is evaluated once per iteration but
//! shares a single `Arc` body, so one entry serves every closure built from
//! it.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use saule_ast::{Expr, LambdaBody, Module};
use saule_semantic::Bindings;

use crate::fxhash::FxHashMap as HashMap;

/// The exact set of enclosing bindings a lambda body refers to.
#[derive(Clone)]
pub(crate) struct Captures {
    pub names: Rc<[Rc<str>]>,
}

thread_local! {
    /// Body address -> capture set, populated per analysed module.
    ///
    /// The `LambdaBody` is stored alongside purely to keep its `Arc` alive:
    /// a freed allocation could otherwise be reused at the same address by an
    /// unrelated lambda and collide with this key.
    static REGISTRY: RefCell<HashMap<usize, (LambdaBody, Captures)>> =
        RefCell::new(HashMap::default());
}

/// Publish the capture sets for every lambda in `module`.
///
/// Called by whoever ran `saule_semantic::analyze_with_bindings` — the
/// pipeline entry points and the module loader. A module that never gets
/// registered simply falls back to whole-scope capture, which is exactly the
/// behaviour that existed before any of this, so the raw
/// [`run`](crate::run) / [`run_in`](crate::run_in) entry points stay correct
/// for callers that skip analysis.
pub(crate) fn register(module: &Module, bindings: &Bindings) {
    let mut entries: Vec<(usize, LambdaBody, Captures)> = Vec::new();
    saule_ast::visit_exprs(module, &mut |e| {
        let Expr::Lambda { body, .. } = &e.value else {
            return;
        };
        let Some(info) = bindings.function(e.id) else {
            return;
        };
        let mut names: Vec<Rc<str>> = info.upval_names.clone();
        // `self` is not an identifier and so never appears in the upvalue
        // list, but a lambda written inside a method still has to reach the
        // enclosing `self`. The resolver tracks that separately.
        if info.captures_self {
            names.push(Rc::from("self"));
        }
        entries.push((
            body_key(body),
            body.clone(),
            Captures {
                names: Rc::from(names.into_boxed_slice()),
            },
        ));
    });

    REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        for (key, body, captures) in entries {
            r.insert(key, (body, captures));
        }
    });
}

/// The capture set for `body`, or `None` if its module was never registered.
pub(crate) fn lookup(body: &LambdaBody) -> Option<Captures> {
    let key = body_key(body);
    REGISTRY.with(|r| r.borrow().get(&key).map(|(_, c)| c.clone()))
}

/// Identity of a lambda body: the address its `Arc` points at.
///
/// Empty blocks may share one well-known dangling address across every empty
/// `Arc<[T]>`, so two distinct empty bodies can collide. Both capture
/// nothing, which makes the collision unobservable.
fn body_key(body: &LambdaBody) -> usize {
    match body {
        LambdaBody::Expr(e) => Arc::as_ptr(e) as *const u8 as usize,
        LambdaBody::Block(stmts) => stmts.as_ptr() as *const u8 as usize,
    }
}
