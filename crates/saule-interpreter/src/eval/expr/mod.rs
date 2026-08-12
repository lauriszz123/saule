//! Expression evaluation.
//!
//! The entry point is [`eval`]. Heavyweight pieces have been moved into
//! sibling modules:
//!
//! | Module        | Contents                                              |
//! |---------------|-------------------------------------------------------|
//! | [`calls`]     | call/dispatch/super machinery, `bind_params`          |
//! | [`construct`] | `ClassName(...)`, tuple-variant constructors          |
//! | [`match_`]    | `match` evaluation and pattern matching               |
//! | [`members`]   | `obj.field` and `obj[index]` reads                    |

mod calls;
mod cast;
mod construct;
mod match_;
pub(crate) mod members;

pub(crate) use calls::{
    call_function, call_static_method_public, call_value_multi, eval_values, invoke_method_multi,
};
#[allow(unused_imports)]
pub(crate) use members::table_index_to_slot;

use std::cell::RefCell;
use std::rc::Rc;

use saule_ast::{BinOp, Expr, LambdaBody, Spanned, TableEntry, Type};

use crate::env::Environment;
use crate::error::RuntimeError;
use crate::value::{FunctionBody, FunctionObject, Value};

use super::ops;

/// Scope binding key used to remember which class a method body is
/// "speaking for" when it calls `self.super(...)`.
const SUPER_OWNER_BINDING: &str = "__saule_super_owner";

#[derive(Clone)]
pub(crate) enum EvaluatedArg {
    Positional(Value),
    Named {
        name: String,
        value: Value,
    },
    /// A trailing block (`f(spacing: 10) do … end`) written after a named
    /// argument. Positional in every respect except that it binds to the last
    /// free function-typed parameter rather than to the next index — see
    /// [`saule_ast::trailing_block_param`]. A trailing block with no named
    /// arguments alongside it is an ordinary [`EvaluatedArg::Positional`].
    TrailingBlock(Value),
}

pub fn eval(expr: &Spanned<Expr>, env: &Rc<RefCell<Environment>>) -> Result<Value, RuntimeError> {
    let span = expr.span.clone();
    match &expr.value {
        // Unreachable through any entry point that runs code: `Expr::Error`
        // only exists in a tree from `saule_parser::parse_recover`, which
        // the language server uses and the interpreter never calls. An
        // error beats a panic in the one case where that stops holding.
        Expr::Error => Err(RuntimeError::TypeError {
            message: "evaluated an expression that failed to parse".into(),
            span,
        }),

        Expr::Int(n) => Ok(Value::Int(*n)),
        Expr::Float(f) => Ok(Value::Float(*f)),
        Expr::Bool(b) => Ok(Value::Bool(*b)),
        Expr::Str(s) => Ok(Value::Str(Rc::new(s.clone()))),
        Expr::Nil => Ok(Value::Nil),

        Expr::Ident(name) => env.borrow().get(name).ok_or_else(|| {
            // saule-semantic's name resolver should have caught any
            // truly-undefined identifier before we reach evaluation. The
            // only way this branch fires in practice is via the bare
            // low-level `run()` entry point on a module that wasn't
            // checked first — surface that as a TypeError rather than a
            // dead variant.
            RuntimeError::TypeError {
                message: format!(
                    "internal: identifier `{name}` reached evaluation undefined — \
                     `saule_semantic::analyze` was not run on this module"
                ),
                span: span.clone(),
            }
        }),

        Expr::Unary { op, rhs } => {
            let v = eval(rhs, env)?;
            ops::unary(*op, v, span)
        }

        // `x as T` — runtime type test. Yields the value on a match and
        // `nil` otherwise; never throws, because the static type is `T?`
        // and the caller is already obliged to handle the `nil`.
        Expr::Cast { value, ty } => {
            let v = eval(value, env)?;
            Ok(if cast::cast(&v, ty) { v } else { Value::Nil })
        }

        Expr::Binary { op, lhs, rhs } => match op {
            // `and` / `or` short-circuit, so evaluate lazily.
            BinOp::And => {
                let l = eval(lhs, env)?;
                if l.is_truthy() { eval(rhs, env) } else { Ok(l) }
            }
            BinOp::Or => {
                let l = eval(lhs, env)?;
                if l.is_truthy() { Ok(l) } else { eval(rhs, env) }
            }
            // `??` short-circuits: only evaluate RHS when LHS is nil.
            BinOp::Coalesce => {
                let l = eval(lhs, env)?;
                if matches!(l, Value::Nil) {
                    eval(rhs, env)
                } else {
                    Ok(l)
                }
            }
            _ => {
                let l = eval(lhs, env)?;
                let r = eval(rhs, env)?;
                ops::binary(*op, l, r, span)
            }
        },

        Expr::Call { callee, args } => {
            // Dotted calls (`obj.method(args)`) and `self.super(args)` need
            // special handling so we can auto-bind `self` and route to the
            // parent constructor respectively.
            if let Expr::Member { obj, name } = &callee.value {
                if name == "super" {
                    return calls::super_call(obj, args, env, span);
                }
                let receiver = eval(obj, env)?;
                let vs = calls::eval_call_args_pub(args, env)?;
                let out = calls::dispatch_member_call(&receiver, name, &vs, span);
                crate::recycle::give_args(vs);
                return out;
            }

            // `obj?.method(args)` — short-circuit the *entire* call when the
            // receiver is nil. Without this special case the inner
            // `SafeMember` would evaluate to nil and we'd fall through to
            // `call_value_pub(nil, ...)`, producing a spurious "value of
            // type `nil` is not callable" error. The expected behaviour is
            // for the whole `p?.foo()` to produce nil, so `p?.foo() ?? x`
            // falls back to `x`.
            if let Expr::SafeMember { obj, name } = &callee.value {
                let receiver = eval(obj, env)?;
                if matches!(receiver, Value::Nil) {
                    return Ok(Value::Nil);
                }
                let vs = calls::eval_call_args_pub(args, env)?;
                let out = calls::dispatch_member_call(&receiver, name, &vs, span);
                crate::recycle::give_args(vs);
                return out;
            }

            let cv = eval(callee, env)?;
            let vs = calls::eval_call_args_pub(args, env)?;
            let out = calls::call_value_pub(cv, &vs, span);
            crate::recycle::give_args(vs);
            out
        }

        Expr::Member { obj, name } => {
            let receiver = eval(obj, env)?;
            members::read_member(&receiver, name, span)
        }
        Expr::SafeMember { obj, name } => {
            let receiver = eval(obj, env)?;
            if matches!(receiver, Value::Nil) {
                Ok(Value::Nil)
            } else {
                members::read_member(&receiver, name, span)
            }
        }
        Expr::Index { obj, index } => {
            let receiver = eval(obj, env)?;
            let index_value = eval(index, env)?;
            members::read_index(&receiver, index_value, span)
        }
        Expr::ForceUnwrap(inner) => {
            let v = eval(inner, env)?;
            if matches!(v, Value::Nil) {
                Err(RuntimeError::ForceUnwrapNil { span })
            } else {
                Ok(v)
            }
        }
        Expr::Table(items) => {
            // Build the array part from positional entries in order, then
            // apply field entries via `set` so the existing array/map split
            // logic (positive contiguous ints land in the array part) is
            // reused uniformly.
            let mut table = crate::value::TableObject::new();
            for item in items {
                match item {
                    TableEntry::Positional(e) => {
                        let v = eval(e, env)?;
                        table.array.push(v);
                    }
                    TableEntry::Field { key, value } => {
                        let k = eval(key, env)?;
                        let v = eval(value, env)?;
                        table.set(&k, v).map_err(|msg| RuntimeError::TypeError {
                            message: msg,
                            span: key.span.clone(),
                        })?;
                    }
                }
            }
            Ok(Value::Table(Rc::new(RefCell::new(table))))
        }

        Expr::Lambda { params, body, .. } => {
            // Capture the names the body mentions rather than the scope
            // itself: holding the scope is a reference cycle the moment the
            // lambda is stored back into it. See `Environment::capture_flat`.
            let closure = match crate::capture::captured_names(body) {
                Some(names) => Environment::capture_flat(env, &names),
                // The body holds something the capture analysis does not
                // model, so fall back to the old whole-scope capture —
                // correct, just not collectable.
                None => env.clone(),
            };
            let body = match body {
                LambdaBody::Expr(e) => FunctionBody::Expr(e.clone()),
                LambdaBody::Block(stmts) => FunctionBody::Block(stmts.clone()),
            };
            Ok(Value::Function(Rc::new(FunctionObject {
                name: None,
                param_keys: FunctionObject::intern_params(params),
                params: params.clone(),
                body,
                closure,
                owner_class: std::cell::RefCell::new(None),
                self_name: std::cell::RefCell::new(None),
                source: crate::module::active_module_source(),
            })))
        }

        Expr::Self_ => env
            .borrow()
            .get("self")
            .ok_or_else(|| RuntimeError::TypeError {
                message: "internal: `self` reached evaluation outside a method — \
                      `saule_semantic::analyze` was not run on this module"
                    .to_string(),
                span,
            }),

        Expr::Match { scrutinee, arms } => match_::eval_match(scrutinee, arms, env, span),

        // `when(source):stage1(args):stage2(args)…` — colon-based pipeline.
        // Each stage threads the upstream value in as the first argument
        // of an otherwise-normal function call. Implemented as a tight
        // loop over `stages` so we never allocate intermediate AST nodes.
        Expr::Pipe { source, stages } => {
            let mut current = eval(source, env)?;
            for stage in stages {
                // Resolve the stage function as a regular identifier — same
                // lookup rules as a bare `name()` call, so locals, globals,
                // and top-level `fn` definitions all work.
                let callee = env.borrow().get(&stage.name).ok_or_else(|| {
                    RuntimeError::TypeError {
                        message: format!(
                            "pipeline stage `{}` is not defined — `when(...):{}` needs a free function in scope",
                            stage.name, stage.name
                        ),
                        span: stage.span.clone(),
                    }
                })?;
                // Evaluate the explicit args, then prepend the piped value
                // as positional arg #0.
                let mut evaled: Vec<EvaluatedArg> = Vec::with_capacity(stage.args.len() + 1);
                evaled.push(EvaluatedArg::Positional(current));
                for a in calls::eval_call_args_pub(&stage.args, env)? {
                    evaled.push(a);
                }
                current = calls::call_value_pub(callee, &evaled, stage.span.clone())?;
            }
            Ok(current)
        }
    }
}

/// Narrow a call's multi-value result down to the single value an expression
/// context wants, and hand the carrier back to the free list.
///
/// This is where the overwhelming majority of result vectors die, which makes
/// it the natural place to recycle them — see [`crate::recycle`].
fn first_or_nil(mut values: Vec<Value>) -> Value {
    let first = if values.is_empty() {
        Value::Nil
    } else {
        values.swap_remove(0)
    };
    crate::recycle::give_values(values);
    first
}

fn is_nullable_type(ty: &Type) -> bool {
    matches!(ty, Type::Nullable(_)) || matches!(ty, Type::Named(n) if n == "nil")
}
