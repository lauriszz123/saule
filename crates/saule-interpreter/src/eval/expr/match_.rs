//! `match` evaluation and pattern matching.

use std::cell::RefCell;
use std::rc::Rc;

use saule_ast::{Expr, MatchArm, MatchBody, Pattern, Spanned};

use crate::env::Environment;
use crate::error::RuntimeError;
use crate::value::Value;

use super::super::Flow;
use super::{eval, eval_values};

/// Evaluate a `match` expression. The scrutinee is evaluated *once*, as a
/// multi-value list so tuple patterns can destructure multi-return calls.
/// Arms are tried top-down; the first whose pattern matches **and** whose
/// guard (if any) holds wins, and its body is evaluated in a fresh scope
/// that contains the pattern's bindings.
pub(super) fn eval_match(
    scrutinee: &Spanned<Expr>,
    arms: &[MatchArm],
    env: &Rc<RefCell<Environment>>,
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    let values = eval_values(scrutinee, env)?;
    let first = values.first().cloned().unwrap_or(Value::Nil);
    for arm in arms {
        let mut bindings: Vec<(String, Value)> = Vec::new();
        if !match_pattern(&arm.pattern.value, &first, &values, &mut bindings) {
            continue;
        }
        let arm_scope = Environment::with_parent(env.clone());
        for (name, value) in &bindings {
            arm_scope.borrow_mut().define(name.clone(), value.clone());
        }
        if let Some(guard) = &arm.guard {
            let g = eval(guard, &arm_scope)?;
            if !g.is_truthy() {
                continue;
            }
        }
        return match &arm.body {
            MatchBody::Expr(e) => eval(e, &arm_scope),
            MatchBody::Block(stmts) => {
                let flow = crate::eval::stmt::exec_block(stmts, &arm_scope)?;
                match flow {
                    Flow::Normal(v) => Ok(v),
                    // `return` / `break` / `continue` from inside an arm
                    // body escapes the match. We park the flow in a
                    // thread-local and signal via `PendingFlow`; the
                    // statement executor at the next boundary picks it
                    // back up and resumes propagation.
                    other => {
                        crate::eval::stmt::pending_flow::set(other);
                        Err(RuntimeError::PendingFlow { span })
                    }
                }
            }
        };
    }
    Err(RuntimeError::TypeError {
        message: "non-exhaustive `match`: no arm matched the value".to_string(),
        span,
    })
}

/// Try to match `pattern` against either the first scrutinee value (`first`)
/// or, for tuple patterns, the full multi-return list (`values`). Returns
/// `true` and appends bindings on success.
fn match_pattern(
    pattern: &Pattern,
    first: &Value,
    values: &[Value],
    out: &mut Vec<(String, Value)>,
) -> bool {
    match pattern {
        Pattern::Wildcard => true,
        Pattern::Bind(name) => {
            out.push((name.clone(), first.clone()));
            true
        }
        Pattern::Nil => matches!(first, Value::Nil),
        Pattern::Int(n) => matches!(first, Value::Int(v) if v == n),
        Pattern::Float(f) => matches!(first, Value::Float(v) if v == f),
        Pattern::Bool(b) => matches!(first, Value::Bool(v) if v == b),
        Pattern::Str(s) => matches!(first, Value::Str(v) if v.as_str() == s.as_str()),
        Pattern::Variant {
            enum_name,
            variant,
            fields,
        } => {
            let Value::EnumVariant(v) = first else {
                return false;
            };
            if v.enum_name != *enum_name || v.variant_name != *variant {
                return false;
            }
            if fields.is_empty() {
                return true;
            }
            let payload: Vec<Value> = match &v.value {
                Some(Value::Table(t)) => t.borrow().array.clone(),
                Some(other) => vec![other.clone()],
                None => Vec::new(),
            };
            if payload.len() != fields.len() {
                return false;
            }
            for (sub, val) in fields.iter().zip(payload.iter()) {
                if !match_pattern(&sub.value, val, std::slice::from_ref(val), out) {
                    out.clear();
                    return false;
                }
            }
            true
        }
        Pattern::Tuple(elems) => {
            if values.len() < elems.len() {
                return false;
            }
            for (sub, val) in elems.iter().zip(values.iter()) {
                if !match_pattern(&sub.value, val, std::slice::from_ref(val), out) {
                    out.clear();
                    return false;
                }
            }
            true
        }
    }
}
