//! Expression evaluation.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use saule_ast::{BinOp, CallArg, Expr, LambdaBody, Spanned, Type};

use crate::env::Environment;
use crate::error::{RuntimeError, unsupported};
use crate::value::{ClassObject, FunctionBody, FunctionObject, InstanceObject, Value};

use super::{Flow, ops};

const SUPER_OWNER_BINDING: &str = "__saule_super_owner";

#[derive(Clone)]
pub(crate) enum EvaluatedArg {
    Positional(Value),
    Named { name: String, value: Value },
}

pub fn eval(expr: &Spanned<Expr>, env: &Rc<RefCell<Environment>>) -> Result<Value, RuntimeError> {
    let span = expr.span.clone();
    match &expr.value {
        Expr::Int(n) => Ok(Value::Int(*n)),
        Expr::Float(f) => Ok(Value::Float(*f)),
        Expr::Bool(b) => Ok(Value::Bool(*b)),
        Expr::Str(s) => Ok(Value::Str(Rc::new(s.clone()))),
        Expr::Nil => Ok(Value::Nil),

        Expr::Ident(name) => env
            .borrow()
            .get(name)
            .ok_or_else(|| RuntimeError::Undefined {
                name: name.clone(),
                span,
            }),

        Expr::Unary { op, rhs } => {
            let v = eval(rhs, env)?;
            ops::unary(*op, v, span)
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
            _ => {
                let l = eval(lhs, env)?;
                let r = eval(rhs, env)?;
                ops::binary(*op, l, r, span)
            }
        },

        Expr::Call { callee, args } => {
            // Dotted calls (`obj.method(args)`) and `self.super(args)` need
            // special handling so we can auto-bind `self` and route to the
            // parent constructor respectively. Saule lets instance methods
            // omit the explicit `self` parameter — see `call_instance_method`.
            if let Expr::Member { obj, name } = &callee.value {
                if name == "super" {
                    return super_call(obj, args, env, span);
                }
                let receiver = eval(obj, env)?;
                let vs = eval_call_args(args, env)?;
                return dispatch_member_call(&receiver, name, vs, span);
            }

            let cv = eval(callee, env)?;
            let vs = eval_call_args(args, env)?;
            call_value(cv, &vs, span)
        }

        // Postfix / composite forms — wait until later phases.
        Expr::Member { obj, name } => {
            let receiver = eval(obj, env)?;
            read_member(&receiver, name, span)
        }
        Expr::SafeMember { .. } => Err(unsupported("safe member access", span)),
        Expr::Index { .. } => Err(unsupported("indexing", span)),
        Expr::MethodCall { obj, method, args } => {
            let receiver = eval(obj, env)?;
            let evaled = eval_call_args(args, env)?;
            invoke_method(&receiver, method, evaled, span)
        }
        Expr::ForceUnwrap(_) => Err(unsupported("force unwrap", span)),
        Expr::New { class, args } => {
            let class_val = env
                .borrow()
                .get(class)
                .ok_or_else(|| RuntimeError::Undefined {
                    name: class.clone(),
                    span: span.clone(),
                })?;
            let class = match class_val {
                Value::Class(c) => c,
                other => {
                    return Err(RuntimeError::TypeError {
                        message: format!(
                            "`new` requires a class name, but got a `{}` — did you mean to call a function instead?",
                            other.type_name()
                        ),
                        span,
                    });
                }
            };
            let mut vs = Vec::with_capacity(args.len());
            for a in args {
                vs.push(EvaluatedArg::Positional(eval(a, env)?));
            }
            construct(class, &vs, span)
        }
        Expr::Table(_) => Err(unsupported("table literal", span)),

        Expr::Lambda { params, body, .. } => {
            let body = match body {
                LambdaBody::Expr(e) => FunctionBody::Expr(e.clone()),
                LambdaBody::Block(stmts) => FunctionBody::Block(stmts.clone()),
            };
            Ok(Value::Function(Rc::new(FunctionObject {
                name: None,
                params: params.clone(),
                body,
                closure: env.clone(),
            })))
        }

        Expr::Self_ => env
            .borrow()
            .get("self")
            .ok_or_else(|| RuntimeError::Undefined {
                name: "self".to_string(),
                span,
            }),
        Expr::Super => Err(unsupported("`super`", span)),
    }
}

/// Invoke a callable [`Value`]. Handles both native and user-defined
/// functions.
fn call_value(
    callee: Value,
    args: &[EvaluatedArg],
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    match callee {
        Value::Native(nf) => {
            let mut positional = Vec::with_capacity(args.len());
            for arg in args {
                match arg {
                    EvaluatedArg::Positional(v) => positional.push(v.clone()),
                    EvaluatedArg::Named { name: _, .. } => {
                        return Err(RuntimeError::TypeError {
                            message: format!(
                                "named arguments are not supported for built-in function `{}` — use positional arguments instead",
                                nf.name
                            ),
                            span,
                        });
                    }
                }
            }
            (nf.func)(&positional).map_err(|message| RuntimeError::TypeError { message, span })
        }
        Value::Function(f) => call_function(&f, args, span),
        // `ClassName(args)` is sugar for `new ClassName(args)`. The explicit
        // `new` form still works through `Expr::New`.
        Value::Class(c) => construct(c, args, span),
        other => Err(RuntimeError::TypeError {
            message: format!(
                "value of type `{}` is not callable — only functions, classes, and methods can be called",
                other.type_name()
            ),
            span,
        }),
    }
}

/// Bind `params` into `scope` from `args`, evaluating defaults lazily. Used
/// by every kind of call (free function, instance method, static method,
/// constructor) so the rules stay in one place.
fn bind_params(
    scope: &Rc<RefCell<Environment>>,
    params: &[saule_ast::Param],
    args: &[EvaluatedArg],
    span: &std::ops::Range<usize>,
) -> Result<(), RuntimeError> {
    for param in params {
        if param.variadic {
            return Err(unsupported("variadic parameters", param.span.clone()));
        }
    }

    let mut assigned: Vec<Option<Value>> = vec![None; params.len()];
    let mut next_positional = 0usize;
    let mut seen_named = false;
    let mut named_arg_count = 0usize;

    for arg in args {
        match arg {
            EvaluatedArg::Positional(value) => {
                if seen_named {
                    return Err(RuntimeError::TypeError {
                        message: format!(
                            "positional argument #{} cannot follow named arguments; all positional arguments must come first",
                            next_positional + 1
                        ),
                        span: span.clone(),
                    });
                }
                if next_positional >= params.len() {
                    return Err(RuntimeError::TypeError {
                        message: format!(
                            "too many arguments: expected {} but got at least {}",
                            params.len(),
                            next_positional + 1
                        ),
                        span: span.clone(),
                    });
                }
                assigned[next_positional] = Some(value.clone());
                next_positional += 1;
            }
            EvaluatedArg::Named { name, value } => {
                seen_named = true;
                named_arg_count += 1;
                let Some(idx) = params.iter().position(|p| p.name == *name) else {
                    let valid_params: Vec<String> = params.iter().map(|p| p.name.clone()).collect();
                    return Err(RuntimeError::TypeError {
                        message: format!(
                            "unknown named argument `{name}` (argument #{} of {}) — valid parameters: {}",
                            named_arg_count,
                            args.len(),
                            valid_params.join(", ")
                        ),
                        span: span.clone(),
                    });
                };
                if assigned[idx].is_some() {
                    return Err(RuntimeError::TypeError {
                        message: format!(
                            "duplicate argument for parameter `{name}` — this parameter was already provided"
                        ),
                        span: span.clone(),
                    });
                }
                assigned[idx] = Some(value.clone());
            }
        }
    }

    let missing_required: Vec<String> = params
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            if assigned[i].is_none()
                && p.default.is_none()
                && !is_nullable_type(&p.ty)
            {
                Some(p.name.clone())
            } else {
                None
            }
        })
        .collect();

    for (i, param) in params.iter().enumerate() {
        let value = if let Some(v) = assigned[i].clone() {
            v
        } else if let Some(default) = &param.default {
            eval(default, scope)?
        } else if is_nullable_type(&param.ty) {
            Value::Nil
        } else {
            return Err(RuntimeError::TypeError {
                message: if missing_required.len() == 1 {
                    format!("missing required argument for parameter `{}`", param.name)
                } else {
                    format!(
                        "missing {} required argument(s): {}",
                        missing_required.len(),
                        missing_required.join(", ")
                    )
                },
                span: span.clone(),
            });
        };
        scope.borrow_mut().define(param.name.clone(), value);
    }
    Ok(())
}

fn is_nullable_type(ty: &Type) -> bool {
    matches!(ty, Type::Nullable(_)) || matches!(ty, Type::Named(n) if n == "nil")
}

fn eval_call_args(
    args: &[CallArg],
    env: &Rc<RefCell<Environment>>,
) -> Result<Vec<EvaluatedArg>, RuntimeError> {
    let mut out = Vec::with_capacity(args.len());
    for arg in args {
        match arg {
            CallArg::Positional(expr) => out.push(EvaluatedArg::Positional(eval(expr, env)?)),
            CallArg::Named { name, value } => out.push(EvaluatedArg::Named {
                name: name.clone(),
                value: eval(value, env)?,
            }),
        }
    }
    Ok(out)
}

fn run_function_body(
    f: &FunctionObject,
    scope: &Rc<RefCell<Environment>>,
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    let outcome = match &f.body {
        FunctionBody::Block(stmts) => super::stmt::exec_block(stmts, scope)?,
        FunctionBody::Expr(e) => Flow::Return(eval(e, scope)?),
    };
    match outcome {
        Flow::Return(v) => Ok(v),
        Flow::Normal(_) => Ok(Value::Nil),
        Flow::Break => Err(RuntimeError::LoopControlOutsideLoop {
            which: "break",
            span,
        }),
        Flow::Continue => Err(RuntimeError::LoopControlOutsideLoop {
            which: "continue",
            span,
        }),
    }
}

/// Skip a leading `self` parameter (so explicit-self style still works
/// alongside the implicit-self style this language prefers).
fn user_params(f: &FunctionObject) -> &[saule_ast::Param] {
    if f.params.first().map(|p| p.name == "self").unwrap_or(false) {
        &f.params[1..]
    } else {
        &f.params
    }
}

/// Call a user-defined function: binds args (with defaults), executes the
/// body in a fresh scope parented to the function's closure, and converts
/// the resulting [`Flow`] into a return value.
pub(crate) fn call_function(
    f: &FunctionObject,
    args: &[EvaluatedArg],
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    let scope = Environment::with_parent(f.closure.clone());
    bind_params(&scope, &f.params, args, &span)?;
    run_function_body(f, &scope, span)
}

/// Invoke an instance method with `self` bound to `receiver`. The method's
/// signature may optionally start with an explicit `self` parameter, which
/// we strip so it's not re-bound from `args`.
fn call_instance_method(
    f: &FunctionObject,
    receiver: Value,
    args: &[EvaluatedArg],
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    let scope = Environment::with_parent(f.closure.clone());
    // Inject the class's static fields/methods so the method body can refer
    // to them by their bare names — see `inject_class_statics` for why.
    if let Value::Instance(inst) = &receiver {
        inject_class_statics(&scope, &inst.borrow().class);
    }
    scope.borrow_mut().define("self".to_string(), receiver);
    bind_params(&scope, user_params(f), args, &span)?;
    run_function_body(f, &scope, span)
}

/// Invoke a static method with `self` bound to the class itself, which is
/// what makes `self.staticField` inside a `static fn` resolve to the right
/// thing (and is the only way `Main.main()` can reach `Main.lauris`).
fn call_static_method(
    f: &FunctionObject,
    class: &Rc<ClassObject>,
    args: &[EvaluatedArg],
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    let scope = Environment::with_parent(f.closure.clone());
    inject_class_statics(&scope, class);
    scope
        .borrow_mut()
        .define("self".to_string(), Value::Class(class.clone()));
    bind_params(&scope, user_params(f), args, &span)?;
    run_function_body(f, &scope, span)
}

/// Make the class's static fields and methods directly visible inside a
/// method body, so users can write `lauris.introduce()` instead of
/// `self.lauris.introduce()` or `Main.lauris.introduce()`. Parent statics
/// are seeded first and overridden by child statics, matching the lookup
/// order used for member access. Locals and parameters bound later still
/// shadow these, so the user can introduce a same-named local without
/// surprise.
fn inject_class_statics(scope: &Rc<RefCell<Environment>>, class: &Rc<ClassObject>) {
    // Collect the chain root-first so the most-derived class wins.
    let mut chain: Vec<Rc<ClassObject>> = Vec::new();
    let mut cur = Some(class.clone());
    while let Some(c) = cur {
        chain.push(c.clone());
        cur = c.parent.clone();
    }
    for c in chain.iter().rev() {
        for (n, v) in c.static_fields.borrow().iter() {
            scope.borrow_mut().define(n.clone(), v.clone());
        }
        for (n, f) in &c.static_methods {
            scope
                .borrow_mut()
                .define(n.clone(), Value::Function(f.clone()));
        }
    }
}

/// Public re-export of [`call_static_method`] for embedders. Kept as a thin
/// wrapper so the implementation details (param binding, `self` injection)
/// stay private to this module.
pub(crate) fn call_static_method_public(
    f: &FunctionObject,
    class: &Rc<ClassObject>,
    args: &[EvaluatedArg],
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    call_static_method(f, class, args, span)
}

/// Resolve `obj.name(args)` where the lookup intent is to *invoke* the
/// result. Instance methods get implicit `self`; static methods get the
/// class as `self`; non-function fields are simply invoked through
/// [`call_value`].
fn dispatch_member_call(
    receiver: &Value,
    name: &str,
    args: Vec<EvaluatedArg>,
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    match receiver {
        Value::Instance(inst) => {
            let class = inst.borrow().class.clone();
            if let Some(m) = class.lookup_method(name) {
                return call_instance_method(&m, receiver.clone(), &args, span);
            }
            if let Some(m) = class.lookup_static_method(name) {
                return call_static_method(&m, &class, &args, span);
            }
            // Fall back: maybe the instance has a callable field.
            if let Some(v) = inst.borrow().fields.get(name).cloned() {
                return call_value(v, &args, span);
            }
            Err(RuntimeError::TypeError {
                message: format!(
                    "no method or field `{name}` on instance of class `{}` — instance members are case-sensitive",
                    class.name
                ),
                span,
            })
        }
        Value::Class(class) => {
            if let Some(m) = class.lookup_static_method(name) {
                return call_static_method(&m, class, &args, span);
            }
            if let Some(v) = class.lookup_static_field(name) {
                return call_value(v, &args, span);
            }
            Err(RuntimeError::TypeError {
                message: format!(
                    "no static member `{name}` on class `{}` — check the class definition for the correct name",
                    class.name
                ),
                span,
            })
        }
        _ => {
            // Generic: read the field, then call it.
            let v = read_member(receiver, name, span.clone())?;
            call_value(v, &args, span)
        }
    }
}

/// `self.super(args)` — call the parent class's constructor on the current
/// instance. The receiver expression must evaluate to an instance whose
/// class has a parent.
fn super_call(
    obj: &Spanned<Expr>,
    args: &[CallArg],
    env: &Rc<RefCell<Environment>>,
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    let receiver = eval(obj, env)?;
    let inst = match receiver {
        Value::Instance(i) => i,
        other => {
            return Err(RuntimeError::TypeError {
                message: format!(
                    "`super(...)` requires `self` (an instance receiver), but got a `{}` — use `self.super(...)` inside a constructor",
                    other.type_name()
                ),
                span,
            });
        }
    };
    let owner_class = super_owner_class(env).unwrap_or_else(|| inst.borrow().class.clone());
    let parent = owner_class
        .parent
        .clone()
        .ok_or_else(|| RuntimeError::TypeError {
            message: format!(
                "class `{}` has no parent to call `super` on",
                owner_class.name
            ),
            span: span.clone(),
        })?;
    let ctor = constructor_chain(&parent).ok_or_else(|| RuntimeError::TypeError {
        message: format!("parent class `{}` has no constructor", parent.name),
        span: span.clone(),
    })?;

    let vs = eval_super_args(args, env, &span)?;

    let scope = Environment::with_parent(ctor.closure.clone());
    scope
        .borrow_mut()
        .define("self".to_string(), Value::Instance(inst));
    scope.borrow_mut().define(
        SUPER_OWNER_BINDING.to_string(),
        Value::Class(parent.clone()),
    );
    bind_params(&scope, user_params(&ctor), &vs, &span)?;
    run_function_body(&ctor, &scope, span).map(|_| Value::Nil)
}

fn eval_super_args(
    args: &[CallArg],
    env: &Rc<RefCell<Environment>>,
    span: &std::ops::Range<usize>,
) -> Result<Vec<EvaluatedArg>, RuntimeError> {
    let mut out = Vec::with_capacity(args.len());
    for arg in args {
        match arg {
            CallArg::Positional(expr) => out.push(EvaluatedArg::Positional(eval(expr, env)?)),
            CallArg::Named { .. } => {
                return Err(RuntimeError::TypeError {
                    message: "named arguments are not supported in `super(...)`".to_string(),
                    span: span.clone(),
                });
            }
        }
    }
    Ok(out)
}

fn super_owner_class(env: &Rc<RefCell<Environment>>) -> Option<Rc<ClassObject>> {
    match env.borrow().get(SUPER_OWNER_BINDING) {
        Some(Value::Class(c)) => Some(c),
        _ => None,
    }
}

// ─── Class / instance helpers ───────────────────────────────────────────────

/// Read `receiver.name`.
///
/// On an instance:
///   1. instance fields,
///   2. class methods (returned as a `Function` — useful for `obj.method`
///      access, though most call sites use the `obj:method()` form),
///   3. class static fields (so `instance.maxHealth` still works the same
///      as `Class.maxHealth`).
///
/// On a class:
///   1. static fields,
///   2. static methods.
fn read_member(
    receiver: &Value,
    name: &str,
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    match receiver {
        Value::Instance(inst) => {
            let inst_ref = inst.borrow();
            if let Some(v) = inst_ref.fields.get(name) {
                return Ok(v.clone());
            }
            if let Some(m) = inst_ref.class.lookup_method(name) {
                return Ok(Value::Function(m));
            }
            if let Some(v) = inst_ref.class.lookup_static_field(name) {
                return Ok(v);
            }
            if let Some(m) = inst_ref.class.lookup_static_method(name) {
                return Ok(Value::Function(m));
            }
            Err(RuntimeError::TypeError {
                message: format!(
                    "no field or method `{name}` on instance of class `{}` — available fields: (check class definition)",
                    inst_ref.class.name
                ),
                span,
            })
        }
        Value::Class(class) => {
            if let Some(v) = class.lookup_static_field(name) {
                return Ok(v);
            }
            if let Some(m) = class.lookup_static_method(name) {
                return Ok(Value::Function(m));
            }
            Err(RuntimeError::TypeError {
                message: format!(
                    "no static member `{name}` on class `{}` — try `{}:` method notation or check if this is an instance method",
                    class.name,
                    class.name
                ),
                span,
            })
        }
        other => Err(RuntimeError::TypeError {
            message: format!(
                "cannot read field `{name}` on value of type `{}` — only instances and classes have members",
                other.type_name()
            ),
            span,
        }),
    }
}

/// Dispatch `receiver:name(args)`. For instances we prepend `self`; for
/// classes we call the static method as-is.
/// Dispatch `receiver:name(args)` (the colon-call form). Delegates to the
/// shared [`dispatch_member_call`] so the dot and colon forms behave the
/// same way at runtime.
fn invoke_method(
    receiver: &Value,
    name: &str,
    args: Vec<EvaluatedArg>,
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    dispatch_member_call(receiver, name, args, span)
}

/// `new Class(args)` — create an instance, populate field defaults, then
/// run the constructor (if any) with `self` bound to the new object.
pub(crate) fn construct(
    class: Rc<ClassObject>,
    args: &[EvaluatedArg],
    span: std::ops::Range<usize>,
) -> Result<Value, RuntimeError> {
    // Allocate the instance up-front so the constructor can stash it as
    // `self` and observe its own writes.
    let inst = Rc::new(RefCell::new(InstanceObject {
        class: class.clone(),
        fields: HashMap::new(),
    }));

    // Initialize instance fields from defaults (walking the inheritance
    // chain parent-first so the most-derived defaults win).
    init_fields(&class, &inst, &span)?;

    // Run the constructor — first declared on this class, otherwise the
    // nearest parent's. If none exists, fields stay at their defaults.
    if let Some(ctor) = constructor_chain(&class) {
        let scope = Environment::with_parent(ctor.closure.clone());
        inject_class_statics(&scope, &class);
        scope
            .borrow_mut()
            .define("self".to_string(), Value::Instance(inst.clone()));
        scope
            .borrow_mut()
            .define(SUPER_OWNER_BINDING.to_string(), Value::Class(class.clone()));
        // Skip a leading explicit `self` parameter if present (the
        // constructor / `init` body uses the auto-bound one).
        bind_params(&scope, user_params(&ctor), args, &span)?;
        run_function_body(&ctor, &scope, span)?;
    }

    Ok(Value::Instance(inst))
}

fn init_fields(
    class: &Rc<ClassObject>,
    inst: &Rc<RefCell<InstanceObject>>,
    span: &std::ops::Range<usize>,
) -> Result<(), RuntimeError> {
    if let Some(parent) = &class.parent {
        init_fields(parent, inst, span)?;
    }
    // Field-default expressions evaluate in the class's declaration scope.
    // We approximate by using the constructor's closure (every method on a
    // class shares that closure), falling back to a fresh prelude-less env
    // when none exists.
    let scope = if let Some(ctor) = &class.constructor {
        Environment::with_parent(ctor.closure.clone())
    } else if let Some(m) = class.methods.values().next() {
        Environment::with_parent(m.closure.clone())
    } else if let Some(m) = class.static_methods.values().next() {
        Environment::with_parent(m.closure.clone())
    } else {
        Environment::new()
    };
    // `self` is visible to default expressions too.
    scope
        .borrow_mut()
        .define("self".to_string(), Value::Instance(inst.clone()));

    for field in &class.field_defs {
        let value = match &field.default {
            Some(e) => eval(e, &scope)?,
            None => Value::Nil,
        };
        inst.borrow_mut().fields.insert(field.name.clone(), value);
    }
    Ok(())
}

fn constructor_chain(class: &Rc<ClassObject>) -> Option<Rc<FunctionObject>> {
    if let Some(c) = &class.constructor {
        return Some(c.clone());
    }
    class.parent.as_ref().and_then(constructor_chain)
}
