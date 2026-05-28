//! Expression-level checks: nullable-receiver detection, private-member
//! access, native-call argument checking, lightweight type inference, and
//! the `?? != nil` style flow narrowing.

use saule_ast::{BinOp, CallArg, Expr, LambdaBody, Spanned, TableEntry, Type};

use super::TypeCheckError;
use super::matches::check_match;
use super::state::{
    Scope, class_implements, current_class, is_interface, is_subtype_named, is_type_param,
    lookup_member, with_classes,
};
use super::stmt::{check_stmt, seed_params};
use super::to_source_span;

// ──────────────────────────────────────────────────────────────────────────────
// Expression checker — walks expressions looking for `obj.member` /
// `obj.method(...)` where `obj` has a statically-known nullable type.
// ──────────────────────────────────────────────────────────────────────────────

pub(super) fn check_expr(
    expr: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    match &expr.value {
        Expr::Member { obj, name } => {
            check_expr(obj, scope, errors);
            report_if_nullable_receiver(obj, name, scope, errors);
            report_if_private(obj, name, scope, errors);
        }
        Expr::MethodCall { obj, method, args } => {
            check_expr(obj, scope, errors);
            report_if_nullable_receiver(obj, method, scope, errors);
            report_if_private(obj, method, scope, errors);
            for a in args {
                check_arg(a, scope, errors);
            }
        }
        Expr::Call { callee, args } => {
            // `obj.method(args)` is parsed as Call(Member { obj, name }, args)
            // — same nullable-receiver rule applies.
            if let Expr::Member { obj, name } = &callee.value {
                check_expr(obj, scope, errors);
                report_if_nullable_receiver(obj, name, scope, errors);
                report_if_private(obj, name, scope, errors);
            } else {
                check_expr(callee, scope, errors);
            }
            for a in args {
                check_arg(a, scope, errors);
            }
            // If the callee resolves to a known native signature, check the
            // argument types positionally. Named arguments are skipped (those
            // aren't supported on natives anyway, and they error at runtime).
            if let Some(qname) = native_callee_name(callee)
                && let Some(sig) = crate::sigs::lookup(&qname)
            {
                check_native_args(&qname, &sig, args, scope, errors, expr.span.clone());
            }
        }
        Expr::SafeMember { obj, .. } => check_expr(obj, scope, errors),
        Expr::Index { obj, index } => {
            check_expr(obj, scope, errors);
            check_expr(index, scope, errors);
        }
        Expr::Unary { rhs, .. } => check_expr(rhs, scope, errors),
        Expr::Binary { lhs, rhs, .. } => {
            check_expr(lhs, scope, errors);
            check_expr(rhs, scope, errors);
        }
        Expr::ForceUnwrap(inner) => check_expr(inner, scope, errors),
        Expr::Table(items) => {
            for entry in items {
                match entry {
                    TableEntry::Positional(e) => check_expr(e, scope, errors),
                    TableEntry::Field { key, value } => {
                        check_expr(key, scope, errors);
                        check_expr(value, scope, errors);
                    }
                }
            }
        }
        Expr::Lambda { params, body, .. } => {
            let mut lscope = scope.clone();
            seed_params(&mut lscope, params);
            match body {
                LambdaBody::Expr(e) => check_expr(e, &lscope, errors),
                LambdaBody::Block(stmts) => {
                    for s in stmts {
                        check_stmt(s, &mut lscope, errors);
                    }
                }
            }
        }
        Expr::Match { scrutinee, arms } => {
            check_match(expr, scrutinee, arms, scope, errors);
        }
        _ => {}
    }
}

pub(super) fn check_arg(arg: &CallArg, scope: &Scope, errors: &mut Vec<TypeCheckError>) {
    match arg {
        CallArg::Positional(e) | CallArg::Named { value: e, .. } => check_expr(e, scope, errors),
    }
}

/// Check positional arguments of a native call against the registered
/// signature. Skips named arguments (natives don't support them; the runtime
/// will surface that as an error).
///
/// Heuristics intentionally lenient:
///   * `any` and `T?` parameters accept anything (incl. nil) — they're the
///     "I'll figure it out" slots.
///   * Variadic / over-supplied calls aren't penalised when the declared
///     param list runs out — many natives accept `...rest` (variadic) which
///     isn't expressed in the sig yet.
///   * If `infer` can't produce a type for the argument, we skip silently.
pub(super) fn check_native_args(
    callee: &str,
    sig: &crate::sigs::NativeSig,
    args: &[CallArg],
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
    call_span: std::ops::Range<usize>,
) {
    // Count required positional params (every param up to the first
    // nullable / `any` is required — nullable+`any` slots are optional).
    let required: usize = sig
        .params
        .iter()
        .take_while(|p| !is_nullable(p) && !is_any(p))
        .count();
    let positional: Vec<&CallArg> = args
        .iter()
        .filter(|a| matches!(a, CallArg::Positional(_)))
        .collect();
    if positional.len() < required {
        errors.push(TypeCheckError::NativeArity {
            callee: callee.to_string(),
            expected: required,
            found: positional.len(),
            span: to_source_span(call_span.clone()),
        });
        return;
    }

    // Reject extras when the native is not variadic. Don't bail though —
    // continue checking the known positions for type mismatches.
    if sig.variadic.is_none() && positional.len() > sig.params.len() {
        errors.push(TypeCheckError::NativeArity {
            callee: callee.to_string(),
            expected: sig.params.len(),
            found: positional.len(),
            span: to_source_span(call_span),
        });
    }

    for (i, arg) in args.iter().enumerate() {
        // Pick the expected type for slot `i`:
        //   - within declared params: use `params[i]`
        //   - past the end: use the variadic element type (or stop if absent)
        let expected = match sig.params.get(i) {
            Some(t) => t,
            None => match &sig.variadic {
                Some(t) => t,
                None => break,
            },
        };
        let value_expr = match arg {
            CallArg::Positional(e) => e,
            CallArg::Named { .. } => continue,
        };
        if is_any(expected) {
            continue;
        }
        let Some(found_ty) = infer(value_expr, scope) else {
            continue;
        };
        if !types_compatible(expected, &found_ty) {
            errors.push(TypeCheckError::NativeArgTypeMismatch {
                callee: callee.to_string(),
                arg: i + 1,
                expected: type_to_string(expected),
                found: type_to_string(&found_ty),
                span: to_source_span(value_expr.span.clone()),
            });
        }
    }
}

pub(super) fn is_any(t: &Type) -> bool {
    matches!(t, Type::Named(n) if n == "any")
}

fn report_if_nullable_receiver(
    obj: &Spanned<Expr>,
    member: &str,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    if let Some(ty) = infer(obj, scope)
        && is_nullable(&ty)
    {
        errors.push(TypeCheckError::NullableMemberAccess {
            ty: type_to_string(&ty),
            member: member.to_string(),
            span: to_source_span(obj.span.clone()),
        });
    }
}

/// Reject access to `local` (private) members from outside the owning class.
/// `self.foo` is allowed only when the *owning* class of `foo` is the class
/// currently being checked — a private field inherited from a parent is
/// **not** visible to the child.
fn report_if_private(
    obj: &Spanned<Expr>,
    member: &str,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    // Resolve the class name we're reading the member off:
    //   * `self.member` → the current class (lookup walks the parent chain).
    //   * `obj.member` where `obj` is an *instance* → infer the receiver type.
    //   * `Class.member` where the receiver is the class itself
    //     (e.g. `Bank.secret`) → use the ident directly.
    let class_name = match &obj.value {
        Expr::Self_ => match current_class() {
            Some(n) => n,
            None => return,
        },
        Expr::Ident(n) if with_classes(|reg| reg.contains_key(n)) => n.clone(),
        _ => match infer(obj, scope) {
            Some(ty) => match strip_nullable(ty) {
                Type::Named(n) => n,
                _ => return,
            },
            None => return,
        },
    };
    let Some((owning, is_private)) = lookup_member(&class_name, member) else {
        return;
    };
    if is_private && current_class().as_deref() != Some(owning.as_str()) {
        errors.push(TypeCheckError::PrivateMemberAccess {
            class: owning,
            member: member.to_string(),
            span: to_source_span(obj.span.end..obj.span.end + member.len() + 1),
        });
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Assignment compatibility — only flags the cases we can prove are wrong.
// ──────────────────────────────────────────────────────────────────────────────

pub(super) fn check_assignment_compat(
    decl_ty: &Type,
    value: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    if is_nullable(decl_ty) {
        return;
    }
    if matches!(value.value, Expr::Nil) {
        errors.push(TypeCheckError::NilToNonNullable {
            ty: type_to_string(decl_ty),
            span: to_source_span(value.span.clone()),
        });
        return;
    }

    // Table-aware checks for table literals assigned to a typed table.
    // Splits the literal into its positional and field halves so each can
    // be validated against the declared table shape independently.
    if let (Type::Table { key, value: elem_ty }, Expr::Table(items)) = (decl_ty, &value.value) {
        let has_positional = items.iter().any(|e| matches!(e, TableEntry::Positional(_)));
        let has_field = items.iter().any(|e| matches!(e, TableEntry::Field { .. }));

        // `{a, b, c}` literal cannot fill a map-typed table whose key is not
        // integer-compatible.
        if let Some(k) = key
            && !is_integer_like(k)
            && has_positional
        {
            errors.push(TypeCheckError::TableArrayLiteralForMap {
                key: type_to_string(k),
                value: type_to_string(elem_ty),
                span: to_source_span(value.span.clone()),
            });
            return;
        }
        // Field entries (`name: ...`) require the table to declare a key
        // type compatible with `string`. Array-typed `table<T>` rejects them.
        if has_field {
            let key_ok = match key {
                None => false,
                Some(k) => is_string_like(k),
            };
            if !key_ok {
                errors.push(TypeCheckError::TableArrayLiteralForMap {
                    key: key.as_deref().map(type_to_string).unwrap_or_else(|| "integer".to_string()),
                    value: type_to_string(elem_ty),
                    span: to_source_span(value.span.clone()),
                });
                return;
            }
        }
        // Each value must match the declared value type.
        for item in items {
            match item {
                TableEntry::Positional(e) | TableEntry::Field { value: e, .. } => {
                    check_element_compat(elem_ty, e, scope, errors);
                }
            }
        }
        return;
    }

    if let Some(value_ty) = infer(value, scope)
        && is_nullable(&value_ty)
    {
        errors.push(TypeCheckError::NullableToNonNullable {
            from: type_to_string(&value_ty),
            to: type_to_string(decl_ty),
            span: to_source_span(value.span.clone()),
        });
    }
}

/// Index key must match the table's declared key type.
pub(super) fn check_table_key_compat(
    expected: &Type,
    index: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    if let Some(idx_ty) = infer(index, scope)
        && !types_compatible(expected, &idx_ty)
    {
        errors.push(TypeCheckError::TableKeyTypeMismatch {
            expected: type_to_string(expected),
            found: type_to_string(&idx_ty),
            span: to_source_span(index.span.clone()),
        });
    }
}

/// True for `integer` and `any` — the key types that an array-style literal
/// can satisfy.
fn is_integer_like(ty: &Type) -> bool {
    matches!(ty, Type::Named(n) if n == "integer" || n == "any")
}

/// True for `string` and `any` — the key types that a field-style literal
/// (`name: expr` / `"text": expr`) can satisfy.
fn is_string_like(ty: &Type) -> bool {
    matches!(ty, Type::Named(n) if n == "string" || n == "any")
}

/// Element-of-table compatibility — accepts literals/`Ident`s whose inferred
/// type matches, and stays quiet otherwise (conservative).
pub(super) fn check_element_compat(
    expected: &Type,
    value: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    if let Some(value_ty) = infer(value, scope)
        && !types_compatible(expected, &value_ty)
    {
        errors.push(TypeCheckError::TableElementTypeMismatch {
            expected: type_to_string(expected),
            found: type_to_string(&value_ty),
            span: to_source_span(value.span.clone()),
        });
    }
}

/// Conservative type compatibility — names match (or either is `any`), or
/// `value_ty` is `Nullable` of a compatible inner, etc.
pub(super) fn types_compatible(expected: &Type, value_ty: &Type) -> bool {
    // Suppress `is_interface` unused-warning when state moves; reference here.
    let _ = is_interface;
    match (expected, value_ty) {
        // Same-name primitives, plus `any` on either side, plus `nil` on the
        // value side (nil is universally assignable; nullable-rejection is
        // handled separately by `NullableToNonNullable`).
        (Type::Named(a), Type::Named(b)) => {
            if a == b || a == "any" || b == "any" || b == "nil" {
                return true;
            }
            // Generic type parameters in scope match anything — they're
            // effectively `any` from the body's point of view.
            if is_type_param(a) || is_type_param(b) {
                return true;
            }
            // `number` is the sentinel used in native sigs to mean
            // "integer or float" — accept either.
            if a == "number" && (b == "integer" || b == "float" || b == "number") {
                return true;
            }
            // Class/interface subtyping: a value of type `b` is assignable to
            // a slot of type `a` if `b` is a subtype of `a` (class implements
            // interface, class extends class, interface extends interface).
            if is_subtype_named(b, a) {
                let _ = class_implements; // keep import even if unused later
                return true;
            }
            false
        }
        // `table<any>` (or `table<any, any>`) matches any table — used by
        // native sigs like `pairs(t)` / `Table.insert(t, ...)` to mean
        // "any table".
        (
            Type::Table { key: ek, value: ev },
            Type::Table { key: vk, value: vv },
        ) => {
            let key_ok = match (ek, vk) {
                (None, None) => true,
                (Some(a), Some(b)) => types_compatible(a, b),
                // Cross-shape (`table<T>` vs `table<K, V>`) only when one
                // side is the `any` wildcard.
                _ => is_any(ev),
            };
            key_ok && (is_any(ev) || types_compatible(ev, vv))
        }
        // Expected table, but value is the bare type-name `table`, `any` or
        // `nil` — accept (caller has erased the element type, or it's nil).
        (Type::Table { .. }, Type::Named(n)) if n == "table" || n == "any" || n == "nil" => true,
        // Expected `any` / `table` / `nil` named slot, value is a table —
        // accept (we widen to the named slot).
        (Type::Named(n), Type::Table { .. }) if n == "table" || n == "any" => true,
        (Type::Nullable(a), b) => types_compatible(a, b),
        (a, Type::Nullable(b)) => types_compatible(a, b),
        // Function / Tuple shapes — only equal-shape is strictly compatible,
        // but the checker doesn't track those precisely yet. Accept rather
        // than emit false positives.
        (Type::Function { .. }, Type::Function { .. }) => true,
        (Type::Tuple(_), Type::Tuple(_)) => true,
        // Different kinds (e.g. table vs integer) — reject.
        _ => false,
    }
}

/// Reject `if`/`while`/`until` conditions that the type system can prove are
/// not `boolean`. Conservative: when `infer` can't determine a type, we skip
/// silently so calls and dynamic expressions keep working.
pub(super) fn check_boolean_cond(
    construct: &'static str,
    cond: &Spanned<Expr>,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    let Some(ty) = infer(cond, scope) else {
        return;
    };
    let is_bool = matches!(&ty, Type::Named(n) if n == "boolean" || n == "any");
    if !is_bool {
        errors.push(TypeCheckError::NonBooleanCondition {
            construct,
            found: type_to_string(&ty),
            span: to_source_span(cond.span.clone()),
        });
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Lightweight type inference.
//
// Returns `Some(ty)` only when we can prove the type. Anything we can't see
// through (calls, member reads on unknown classes, indexing) returns `None`,
// and callers treat that as "don't know, don't complain".
// ──────────────────────────────────────────────────────────────────────────────

pub(super) fn infer(expr: &Spanned<Expr>, scope: &Scope) -> Option<Type> {
    match &expr.value {
        Expr::Nil => Some(Type::Named("nil".into())),
        Expr::Int(_) => Some(Type::Named("integer".into())),
        Expr::Float(_) => Some(Type::Named("float".into())),
        Expr::Bool(_) => Some(Type::Named("boolean".into())),
        Expr::Str(_) => Some(Type::Named("string".into())),
        Expr::Ident(n) => scope.lookup(n).cloned(),
        Expr::SafeMember { .. } => Some(Type::Nullable(Box::new(Type::Named("any".into())))),
        Expr::ForceUnwrap(inner) => match infer(inner, scope)? {
            Type::Nullable(t) => Some(*t),
            other => Some(other),
        },
        Expr::Binary { op, lhs, rhs } => match op {
            BinOp::Coalesce => {
                // `lhs ?? rhs`: result is non-nullable iff `rhs` is non-nullable.
                match (infer(lhs, scope), infer(rhs, scope)) {
                    (Some(lt), Some(rt)) => {
                        let base = strip_nullable(lt);
                        if is_nullable(&rt) {
                            Some(Type::Nullable(Box::new(base)))
                        } else {
                            Some(base)
                        }
                    }
                    _ => None,
                }
            }
            BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
                Some(Type::Named("boolean".into()))
            }
            BinOp::And | BinOp::Or => None,
            BinOp::Concat => Some(Type::Named("string".into())),
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                infer(lhs, scope).or_else(|| infer(rhs, scope))
            }
        },
        // `Foo(args)` where `Foo` is a known class → produces a `Foo`.
        // Otherwise, look up the qualified callee name in the native signature
        // table (e.g. `String.byte`, `Math.tointeger`, `assert`).
        Expr::Call { callee, .. } => {
            if let Expr::Ident(n) = &callee.value
                && with_classes(|reg| reg.contains_key(n))
            {
                Some(Type::Named(n.clone()))
            } else if let Some(qname) = native_callee_name(callee)
                && let Some(sig) = crate::sigs::lookup(&qname)
            {
                first_or_tuple(sig.returns)
            } else {
                None
            }
        }
        // A `match` expression has the type of its (unified) arm bodies. We
        // only need an approximation, so return the first arm-body type we
        // can infer.
        Expr::Match { arms, .. } => arms.iter().find_map(|a| match &a.body {
            saule_ast::MatchBody::Expr(e) => infer(e, scope),
            saule_ast::MatchBody::Block(_) => None,
        }),
        _ => None,
    }
}

pub(super) fn strip_nullable(ty: Type) -> Type {
    match ty {
        Type::Nullable(t) => *t,
        other => other,
    }
}

/// Build a qualified callee name suitable for `stdlib::sigs::lookup`:
/// `assert` or `String.byte`.
fn native_callee_name(callee: &Spanned<Expr>) -> Option<String> {
    match &callee.value {
        Expr::Ident(n) => Some(n.clone()),
        Expr::Member { obj, name } => {
            if let Expr::Ident(class) = &obj.value {
                Some(format!("{class}.{name}"))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Collapse a returns-list into a single inferred type: single returns stay
/// as-is, multi-returns become a `Type::Tuple`.
fn first_or_tuple(returns: Vec<Type>) -> Option<Type> {
    match returns.len() {
        0 => None,
        1 => returns.into_iter().next(),
        _ => Some(Type::Tuple(returns)),
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Flow narrowing.
//
// `narrow_truthy(cond, scope)` — apply the assumptions that hold when `cond`
// is true. Today: `x != nil` and `nil != x` strip `Nullable` off `x`. `and`
// chains compose.
//
// `narrow_falsy(cond, scope)` — apply the assumptions when `cond` is false.
// Used by the else-branch: `if x == nil then ... else ... end` narrows `x`
// in the else.
// ──────────────────────────────────────────────────────────────────────────────

pub(super) fn narrow_truthy(cond: &Spanned<Expr>, scope: &mut Scope) {
    match &cond.value {
        Expr::Binary {
            op: BinOp::NotEq,
            lhs,
            rhs,
        } => {
            if let Some(name) = pick_ident_compared_to_nil(lhs, rhs)
                && let Some(Type::Nullable(t)) = scope.lookup(name).cloned()
            {
                scope.bind(name.to_string(), *t);
            }
        }
        Expr::Binary {
            op: BinOp::And,
            lhs,
            rhs,
        } => {
            narrow_truthy(lhs, scope);
            narrow_truthy(rhs, scope);
        }
        _ => {}
    }
}

pub(super) fn narrow_falsy(cond: &Spanned<Expr>, scope: &mut Scope) {
    if let Expr::Binary {
        op: BinOp::Eq,
        lhs,
        rhs,
    } = &cond.value
        && let Some(name) = pick_ident_compared_to_nil(lhs, rhs)
        && let Some(Type::Nullable(t)) = scope.lookup(name).cloned()
    {
        scope.bind(name.to_string(), *t);
    }
}

/// If `lhs` / `rhs` is `(Ident(x), Nil)` in either order, return `Some(x)`.
fn pick_ident_compared_to_nil<'a>(
    lhs: &'a Spanned<Expr>,
    rhs: &'a Spanned<Expr>,
) -> Option<&'a str> {
    match (&lhs.value, &rhs.value) {
        (Expr::Ident(n), Expr::Nil) => Some(n),
        (Expr::Nil, Expr::Ident(n)) => Some(n),
        _ => None,
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Misc helpers.
// ──────────────────────────────────────────────────────────────────────────────

pub(super) fn type_to_string(ty: &Type) -> String {
    match ty {
        Type::Named(n) => n.clone(),
        Type::Nullable(inner) => format!("{}?", type_to_string(inner)),
        Type::Table { key: None, value } => format!("table<{}>", type_to_string(value)),
        Type::Table {
            key: Some(k),
            value,
        } => format!("table<{}, {}>", type_to_string(k), type_to_string(value)),
        other => format!("{:?}", other),
    }
}

pub(super) fn is_nullable(ty: &Type) -> bool {
    match ty {
        Type::Nullable(_) => true,
        Type::Named(n) => n == "nil",
        _ => false,
    }
}
