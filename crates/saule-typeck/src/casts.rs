//! What `x as T` means for a given pair of types.
//!
//! One keyword carries two operations, and this module is the single place
//! that decides which one a given cast is. Both the checker (for the error
//! and the result type) and [`crate::resolve_casts`] (for the [`CastKind`]
//! the runtime reads) go through [`resolve`], so the diagnostic and the
//! behaviour can never disagree.
//!
//! ## The two readings
//!
//! * **Checked** — the operand is `any` or a generic parameter, so its
//!   contents are unknown and the cast is a runtime type test. Result is
//!   `T?`: `nil` when the value wasn't a `T`.
//! * **Convert** — the operand's type is known, so there is nothing to
//!   test; the cast changes the value. Result is `T` when the conversion
//!   cannot fail, `T?` when it can (`"12x" as integer` has to be able to
//!   say no).
//!
//! ## Why the pair table is short
//!
//! Every entry is a conversion whose result is obvious from reading the
//! line. `10f as integer` truncating, `2 as float` widening, and `n as
//! string` rendering are all things a reader predicts without knowing the
//! language. Deliberately absent: `string as boolean` (which of `""`,
//! `"false"`, `"0"` is false?), `boolean as integer` (no such convention
//! here), and anything involving tables, functions, or instances — those
//! have no single defensible answer, so they stay errors and the user
//! writes the mapping they meant.

use saule_ast::{CastKind, Type};

use crate::expr::generics::is_any;
use crate::expr::infer::strip_nullable;
use crate::state::is_type_param;

/// What a cast from `source` to `target` does.
#[derive(Debug, Clone, PartialEq)]
pub enum CastRule {
    /// Runtime type test; result `T?`.
    Checked,
    /// Value conversion that always succeeds; result `T`.
    Convert,
    /// Value conversion that can fail; result `T?`.
    TryConvert,
    /// The operand already has the target's type — nothing to test and
    /// nothing to convert.
    Redundant,
    /// No reading applies.
    Impossible,
}

impl CastRule {
    /// The [`CastKind`] the runtime needs to carry out this rule.
    pub fn kind(&self) -> CastKind {
        match self {
            CastRule::Convert | CastRule::TryConvert => CastKind::Convert,
            // `Redundant` and `Impossible` are errors; the kind only
            // matters if something runs anyway, and the type test is the
            // reading that cannot invent a value.
            _ => CastKind::Checked,
        }
    }

    /// The type `x as T` has under this rule.
    pub fn result(&self, target: &Type) -> Type {
        match self {
            CastRule::Convert => target.clone(),
            _ => nullable(target.clone()),
        }
    }
}

/// Decide `source as target`. `source` is `None` when inference could not
/// prove a type for the operand — the checker's standing "don't know, don't
/// complain" answer, which here means falling back to the type test rather
/// than inventing a conversion.
pub fn resolve(source: Option<&Type>, target: &Type) -> CastRule {
    let Some(source) = source else {
        return CastRule::Checked;
    };

    // A `T?` operand is cast by casting its payload; `nil` passes straight
    // through. That keeps `maybeFloat as integer` legal and honestly typed
    // as `integer?` rather than forcing a `!` before the conversion.
    let bare = strip_nullable(source.clone());
    let nullable_source = matches!(source, Type::Nullable(_));

    // Unknown contents: the only sound reading is a test. A rigid generic
    // parameter is exactly as unknown inside the body as `any` — it stands
    // for whatever the caller chose — so it narrows the same way.
    if is_any(&bare) || matches!(&bare, Type::Named(n) if is_type_param(n)) {
        return CastRule::Checked;
    }

    // `x as T` where `x` is already a `T`. Still worth flagging on a
    // non-nullable operand (it does nothing), but on a `T?` operand the
    // cast is the nil-preserving identity, which is a no-op too.
    if strip_nullable(target.clone()) == bare {
        return CastRule::Redundant;
    }

    let (Type::Named(from), Type::Named(to)) = (&bare, &strip_nullable(target.clone())) else {
        return CastRule::Impossible;
    };
    let to = to.clone();

    let rule = match (from.as_str(), to.as_str()) {
        // Numeric. `float as integer` truncates toward zero and saturates
        // at the `i64` ends rather than wrapping, matching what the old
        // `int()` did to the same values.
        ("integer", "float") | ("float", "integer") => CastRule::Convert,
        // Rendering. The same text `tostring` produces, which is what a
        // reader of `n as string` expects to get.
        ("integer" | "float" | "boolean", "string") => CastRule::Convert,
        // Parsing, which is the one direction that can fail.
        ("string", "integer" | "float") => CastRule::TryConvert,
        _ => CastRule::Impossible,
    };

    // A conversion off a nullable operand inherits the nil.
    match (rule, nullable_source) {
        (CastRule::Convert, true) => CastRule::TryConvert,
        (rule, _) => rule,
    }
}

fn nullable(ty: Type) -> Type {
    match ty {
        // `T?` as a cast target is the same target as `T`; the result is
        // nullable either way, and double-wrapping would print as `T??`.
        Type::Nullable(_) => ty,
        other => Type::Nullable(Box::new(other)),
    }
}

// ── Publishing the decision to the runtime ──────────────────────────────
//
// The rule above is computed while checking, where the operand's type is
// known. The interpreter and the bytecode compiler need the answer later,
// when it is not. So the check records what it decided, keyed by node, and
// [`crate::check_and_resolve`] stamps it into the tree.
//
// Recording is unconditional — unlike the type table, which is opt-in.
// There is one entry per `as` in the program, so the cost is nothing, and
// making it optional would mean a pipeline could typecheck a module and
// still run it with the casts unresolved.

use std::cell::RefCell;
use std::collections::HashMap;

use saule_ast::NodeId;

thread_local! {
    /// Cast node -> the kind the check decided on. Drained per module by
    /// [`crate::check_and_resolve`]; ids are per module, so it must not
    /// outlive the check that filled it.
    static SINK: RefCell<HashMap<NodeId, CastKind>> = RefCell::new(HashMap::new());
}

/// Record `rule`'s kind for the cast node `id`.
pub(crate) fn record(id: NodeId, rule: &CastRule) {
    if id.is_none() {
        return;
    }
    SINK.with(|s| s.borrow_mut().insert(id, rule.kind()));
}

/// Start a fresh recording, handing back whatever was in progress.
pub(crate) fn begin() -> HashMap<NodeId, CastKind> {
    SINK.with(|s| std::mem::take(&mut *s.borrow_mut()))
}

/// Finish a recording, restoring `previous`.
pub(crate) fn end(previous: HashMap<NodeId, CastKind>) -> HashMap<NodeId, CastKind> {
    SINK.with(|s| std::mem::replace(&mut *s.borrow_mut(), previous))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(n: &str) -> Type {
        Type::Named(n.into())
    }

    #[test]
    fn an_unknown_operand_falls_back_to_the_test() {
        assert_eq!(resolve(None, &named("integer")), CastRule::Checked);
        assert_eq!(
            resolve(Some(&named("any")), &named("integer")),
            CastRule::Checked
        );
        // Through a `?` as well: `any?` is still an `any` underneath.
        assert_eq!(
            resolve(
                Some(&Type::Nullable(Box::new(named("any")))),
                &named("integer")
            ),
            CastRule::Checked
        );
    }

    #[test]
    fn the_numeric_pairs_convert_both_ways() {
        assert_eq!(
            resolve(Some(&named("float")), &named("integer")),
            CastRule::Convert
        );
        assert_eq!(
            resolve(Some(&named("integer")), &named("float")),
            CastRule::Convert
        );
    }

    #[test]
    fn rendering_is_total_and_parsing_is_not() {
        for from in ["integer", "float", "boolean"] {
            assert_eq!(
                resolve(Some(&named(from)), &named("string")),
                CastRule::Convert,
                "{from} as string"
            );
        }
        for to in ["integer", "float"] {
            assert_eq!(
                resolve(Some(&named("string")), &named(to)),
                CastRule::TryConvert,
                "string as {to}"
            );
        }
    }

    #[test]
    fn a_nullable_operand_makes_a_total_conversion_fallible() {
        let source = Type::Nullable(Box::new(named("float")));
        assert_eq!(
            resolve(Some(&source), &named("integer")),
            CastRule::TryConvert
        );
    }

    #[test]
    fn the_result_type_follows_the_rule() {
        let int = named("integer");
        assert_eq!(CastRule::Convert.result(&int), int);
        let nullable = Type::Nullable(Box::new(int.clone()));
        assert_eq!(CastRule::TryConvert.result(&int), nullable);
        assert_eq!(CastRule::Checked.result(&int), nullable);
        // `T?` as the written target does not stack into `T??`.
        assert_eq!(CastRule::Checked.result(&nullable), nullable);
    }

    #[test]
    fn casting_to_the_type_already_held_is_redundant_and_off_table_pairs_are_impossible() {
        assert_eq!(
            resolve(Some(&named("integer")), &named("integer")),
            CastRule::Redundant
        );
        assert_eq!(
            resolve(Some(&named("integer")), &named("boolean")),
            CastRule::Impossible
        );
        assert_eq!(
            resolve(Some(&named("string")), &named("boolean")),
            CastRule::Impossible
        );
    }

    #[test]
    fn only_a_conversion_asks_the_runtime_to_convert() {
        assert_eq!(CastRule::Convert.kind(), CastKind::Convert);
        assert_eq!(CastRule::TryConvert.kind(), CastKind::Convert);
        assert_eq!(CastRule::Checked.kind(), CastKind::Checked);
        // The error rules never run, and the test is the reading that
        // cannot invent a value if something runs anyway.
        assert_eq!(CastRule::Impossible.kind(), CastKind::Checked);
    }
}
