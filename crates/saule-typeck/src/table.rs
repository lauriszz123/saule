//! The published type table (`VM_DESIGN.md` §21.1 item 0.5).
//!
//! Today `check` computes a type for most expressions, uses it for one
//! diagnostic, and throws it away. The bytecode compiler needs exactly that
//! information and cannot recompute it — choosing `ADDI` over `ARITHX` *is*
//! the question "did the checker prove both operands are integers?".
//!
//! ## How it is collected
//!
//! Recording hangs off [`infer`](crate::expr::infer) rather than a second
//! pass. `check_stmt` / `check_expr` are plain recursive walks with no
//! context parameter, and the crate already keeps `CURRENT_CLASS`,
//! `RETURN_TY` and friends in thread-locals for that reason — a thread-local
//! sink is consistent with that, and it avoids threading a `&mut TypeTable`
//! through every signature in the crate.
//!
//! The sink is `None` during a plain [`check`](crate::check), so the
//! ordinary path — which the language server runs on every keystroke — pays
//! one thread-local read per `infer` call and allocates nothing.
//!
//! ## What it does *not* contain
//!
//! Coverage is partial by construction. `infer` is documented as
//! "intentionally partial", and it only runs on nodes the checker had a
//! reason to ask about. **Every missing entry is an opcode that degrades to
//! its dynamic form, never a miscompile** — which is why the table is
//! allowed to ship incomplete, and why [`crate::coverage`] exists to say how
//! incomplete.

use std::cell::RefCell;
use std::collections::HashMap;

use saule_ast::{NodeId, Type};

/// Inferred type per AST node, keyed by [`NodeId`].
pub type TypeTable = HashMap<NodeId, Type>;

thread_local! {
    /// `Some` only while `check_with_types` is running.
    static SINK: RefCell<Option<TypeTable>> = const { RefCell::new(None) };
}

/// Record `ty` for `id`.
///
/// **Last write wins.** A node can be inferred more than once — flow
/// narrowing re-infers an identifier inside a proven-non-nil branch, for
/// instance — and the later answer is the narrower one, which is the more
/// useful of the two. Nodes carrying [`NodeId::NONE`] are dropped rather
/// than colliding on a shared key; that only happens for a tree that never
/// went through `saule_ast::assign_ids`.
#[inline]
pub(crate) fn record(id: NodeId, ty: &Type) {
    if id.is_none() {
        return;
    }
    SINK.with(|s| {
        if let Some(table) = s.borrow_mut().as_mut() {
            table.insert(id, ty.clone());
        }
    });
}

/// Install an empty sink for the duration of a check, returning whatever was
/// there before so nested checks restore it.
pub(crate) fn begin() -> Option<TypeTable> {
    SINK.with(|s| s.borrow_mut().replace(TypeTable::new()))
}

/// Take the collected table back out and restore `previous`.
pub(crate) fn end(previous: Option<TypeTable>) -> TypeTable {
    SINK.with(|s| std::mem::replace(&mut *s.borrow_mut(), previous)).unwrap_or_default()
}
