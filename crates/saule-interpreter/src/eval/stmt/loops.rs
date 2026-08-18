//! Numeric and generic `for` loops.

use std::cell::RefCell;
use std::rc::Rc;

use saule_ast::{Expr, Spanned, Stmt};

use crate::env::Environment;
use crate::error::RuntimeError;
use crate::value::Value;

use super::super::{Flow, expr};
use super::exec_block;

pub(super) fn exec_for_numeric(
    var: &str,
    from: &Spanned<Expr>,
    to: &Spanned<Expr>,
    step: Option<&Spanned<Expr>>,
    body: &[Spanned<Stmt>],
    env: &Rc<RefCell<Environment>>,
    span: std::ops::Range<usize>,
) -> Result<Flow, RuntimeError> {
    let from_v = expr::eval(from, env)?;
    let to_v = expr::eval(to, env)?;
    let step_v = match step {
        Some(e) => expr::eval(e, env)?,
        None => match &from_v {
            Value::Float(_) => Value::Float(1.0),
            _ => Value::Int(1),
        },
    };

    match (from_v, to_v, step_v) {
        (Value::Int(f), Value::Int(t), Value::Int(s)) => {
            if s == 0 {
                return Err(RuntimeError::ZeroStep { span });
            }
            run_numeric_loop_int(var, f, t, s, body, env)
        }
        (Value::Float(f), Value::Float(t), Value::Float(s)) => {
            if s == 0.0 {
                return Err(RuntimeError::ZeroStep { span });
            }
            run_numeric_loop_float(var, f, t, s, body, env)
        }
        (f, t, s) => Err(RuntimeError::TypeError {
            message: format!(
                "numeric `for` loop requires all bounds (from, to, step) to be the same numeric type — got `{}`, `{}`, `{}` (use matching integer or float bounds)",
                f.type_name(),
                t.type_name(),
                s.type_name()
            ),
            span,
        }),
    }
}

fn run_numeric_loop_int(
    var: &str,
    from: i64,
    to: i64,
    step: i64,
    body: &[Spanned<Stmt>],
    parent: &Rc<RefCell<Environment>>,
) -> Result<Flow, RuntimeError> {
    // Intern the loop variable's name once. `define` takes an `Rc<str>`, so
    // handing it a `&str` per iteration would allocate a `String` *and* an
    // `Rc` every time round — two heap allocations per iteration for a name
    // that never changes.
    let key: Rc<str> = Rc::from(var);
    let mut i = from;
    let mut scope = Environment::with_parent(parent.clone());
    while (step > 0 && i <= to) || (step < 0 && i >= to) {
        scope.borrow_mut().define(Rc::clone(&key), Value::Int(i));
        match exec_block(body, &scope)? {
            Flow::Normal(_) | Flow::Continue => {}
            Flow::Break => return Ok(Flow::nil()),
            ret @ (Flow::Return(_) | Flow::TailCall { .. }) => return Ok(ret),
        }
        scope = Environment::recycle(scope, parent);
        // Detect overflow so a too-large step doesn't loop forever.
        let (next, overflow) = i.overflowing_add(step);
        if overflow {
            break;
        }
        i = next;
    }
    Ok(Flow::nil())
}

fn run_numeric_loop_float(
    var: &str,
    from: f64,
    to: f64,
    step: f64,
    body: &[Spanned<Stmt>],
    parent: &Rc<RefCell<Environment>>,
) -> Result<Flow, RuntimeError> {
    // Interned once — see `run_numeric_loop_int`.
    let key: Rc<str> = Rc::from(var);
    let mut i = from;
    let mut scope = Environment::with_parent(parent.clone());
    while (step > 0.0 && i <= to) || (step < 0.0 && i >= to) {
        scope.borrow_mut().define(Rc::clone(&key), Value::Float(i));
        match exec_block(body, &scope)? {
            Flow::Normal(_) | Flow::Continue => {}
            Flow::Break => return Ok(Flow::nil()),
            ret @ (Flow::Return(_) | Flow::TailCall { .. }) => return Ok(ret),
        }
        scope = Environment::recycle(scope, parent);
        i += step;
    }
    Ok(Flow::nil())
}

pub(super) fn exec_for_in(
    vars: &[(String, Option<saule_ast::Type>)],
    iter: &Spanned<Expr>,
    body: &[Spanned<Stmt>],
    env: &Rc<RefCell<Environment>>,
    span: std::ops::Range<usize>,
) -> Result<Flow, RuntimeError> {
    // Interned once for the whole loop — see `run_numeric_loop_int`. Both the
    // table path and the closure-driver path below bind these every iteration.
    let keys: Vec<Rc<str>> = vars.iter().map(|(n, _)| Rc::from(n.as_str())).collect();
    let iter_value = expr::eval(iter, env)?;
    match iter_value {
        Value::Table(items) => {
            // Snapshot to allow the table to mutate during iteration without
            // breaking the loop. Yield array entries first, then map entries.
            let (array, map_entries) = {
                let t = items.borrow();
                let array = t.array.clone();
                let mut map_entries: Vec<(crate::value::TableKey, Value)> =
                    t.map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                map_entries.sort_by(|a, b| match (&a.0, &b.0) {
                    (crate::value::TableKey::Int(x), crate::value::TableKey::Int(y)) => x.cmp(y),
                    (crate::value::TableKey::Int(_), _) => std::cmp::Ordering::Less,
                    (_, crate::value::TableKey::Int(_)) => std::cmp::Ordering::Greater,
                    (crate::value::TableKey::Str(x), crate::value::TableKey::Str(y)) => x.cmp(y),
                    (crate::value::TableKey::Str(_), _) => std::cmp::Ordering::Less,
                    (_, crate::value::TableKey::Str(_)) => std::cmp::Ordering::Greater,
                    (crate::value::TableKey::Bool(x), crate::value::TableKey::Bool(y)) => x.cmp(y),
                });
                (array, map_entries)
            };

            let run_iter = |scope: &Rc<RefCell<Environment>>,
                            key: Value,
                            value: Value|
             -> Result<Flow, RuntimeError> {
                match keys.as_slice() {
                    [name] => {
                        scope.borrow_mut().define(Rc::clone(name), value);
                    }
                    [key_name, value_name] => {
                        scope.borrow_mut().define(Rc::clone(key_name), key);
                        scope.borrow_mut().define(Rc::clone(value_name), value);
                    }
                    _ => {
                        return Err(RuntimeError::TypeError {
                            message: format!(
                                "for-in loops support one value variable or a key/value pair, got {} variables",
                                vars.len()
                            ),
                            span: span.clone(),
                        });
                    }
                }
                exec_block(body, scope)
            };

            let mut scope = Environment::with_parent(env.clone());
            for (i, value) in array.into_iter().enumerate() {
                match run_iter(&scope, Value::Int((i + 1) as i64), value)? {
                    Flow::Normal(_) | Flow::Continue => {}
                    Flow::Break => return Ok(Flow::nil()),
                    ret @ (Flow::Return(_) | Flow::TailCall { .. }) => return Ok(ret),
                }
                scope = Environment::recycle(scope, env);
            }
            for (k, v) in map_entries {
                match run_iter(&scope, k.to_value(), v)? {
                    Flow::Normal(_) | Flow::Continue => {}
                    Flow::Break => return Ok(Flow::nil()),
                    ret @ (Flow::Return(_) | Flow::TailCall { .. }) => return Ok(ret),
                }
                scope = Environment::recycle(scope, env);
            }
            Ok(Flow::nil())
        }
        other => {
            // For functions and instances we drive a closure-based iterator.
            // Instances must expose an `iter()` method that returns the closure.
            let driver: Value = match &other {
                Value::Function(_) | Value::Native(_) | Value::NativeClosure(_) => other.clone(),
                Value::Instance(_) => {
                    let result =
                        expr::invoke_method_multi(&other, "iter", Vec::new(), span.clone())?;
                    let Some(driver) = result.into_iter().next() else {
                        return Err(RuntimeError::TypeError {
                            message: format!(
                                "`{}.iter()` returned no value — it must return a function",
                                other.type_name()
                            ),
                            span,
                        });
                    };
                    if !matches!(
                        driver,
                        Value::Function(_) | Value::Native(_) | Value::NativeClosure(_)
                    ) {
                        return Err(RuntimeError::TypeError {
                            message: format!(
                                "`iter()` must return a function, got `{}`",
                                driver.type_name()
                            ),
                            span,
                        });
                    }
                    driver
                }
                _ => {
                    return Err(RuntimeError::TypeError {
                        message: format!(
                            "cannot iterate over a `{}` with `for ... in` — use a table, a function, or a class that implements `Iterable`",
                            other.type_name()
                        ),
                        span,
                    });
                }
            };

            // Drive the closure: call repeatedly with no arguments. Stop when
            // the first returned value is `nil` (Lua's nil-terminator). Each
            // step's returns are bound positionally across the loop variables
            // (extras → nil, surplus values dropped).
            let mut scope = Environment::with_parent(env.clone());
            loop {
                let values = expr::call_value_multi(driver.clone(), &[], span.clone())?;
                if values.first().is_none_or(|v| matches!(v, Value::Nil)) {
                    break;
                }
                {
                    let mut scope_mut = scope.borrow_mut();
                    for (i, name) in keys.iter().enumerate() {
                        let v = values.get(i).cloned().unwrap_or(Value::Nil);
                        scope_mut.define(Rc::clone(name), v);
                    }
                }
                match exec_block(body, &scope)? {
                    Flow::Normal(_) | Flow::Continue => {}
                    Flow::Break => return Ok(Flow::nil()),
                    ret @ (Flow::Return(_) | Flow::TailCall { .. }) => return Ok(ret),
                }
                scope = Environment::recycle(scope, env);
            }
            Ok(Flow::nil())
        }
    }
}
