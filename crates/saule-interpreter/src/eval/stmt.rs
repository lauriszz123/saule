//! Statement execution with control-flow propagation.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use saule_ast::{ClassMember, Decl, Expr, Param, Spanned, Stmt};

use crate::env::Environment;
use crate::error::{RuntimeError, unsupported};
use crate::value::{ClassObject, FieldDef, FunctionBody, FunctionObject, Value};

use super::{Flow, expr};

/// Execute a sequence of statements in `env`. Stops at the first non-`Normal`
/// outcome and propagates it.
pub fn exec_block(
    stmts: &[Spanned<Stmt>],
    env: &Rc<RefCell<Environment>>,
) -> Result<Flow, RuntimeError> {
    let mut last = Flow::nil();
    for stmt in stmts {
        match exec(stmt, env)? {
            Flow::Normal(v) => last = Flow::Normal(v),
            other => return Ok(other),
        }
    }
    Ok(last)
}

/// Run a block in a fresh child scope. The scope is dropped on return.
fn exec_scoped_block(
    stmts: &[Spanned<Stmt>],
    parent: &Rc<RefCell<Environment>>,
) -> Result<Flow, RuntimeError> {
    let scope = Environment::with_parent(parent.clone());
    exec_block(stmts, &scope)
}

/// Execute a single statement.
pub fn exec(stmt: &Spanned<Stmt>, env: &Rc<RefCell<Environment>>) -> Result<Flow, RuntimeError> {
    let span = stmt.span.clone();
    match &stmt.value {
        Stmt::Local { name, value, .. } => {
            let v = match value {
                Some(e) => expr::eval(e, env)?,
                None => Value::Nil,
            };
            env.borrow_mut().define(name.clone(), v);
            Ok(Flow::nil())
        }

        Stmt::LocalMulti { names, values } => {
            // Evaluate every RHS first so `local a, b = b, a` works at the
            // outer scope.
            let mut evaluated = Vec::with_capacity(values.len());
            for v in values {
                evaluated.push(expr::eval(v, env)?);
            }
            for (i, (name, _)) in names.iter().enumerate() {
                let v = evaluated.get(i).cloned().unwrap_or(Value::Nil);
                env.borrow_mut().define(name.clone(), v);
            }
            Ok(Flow::nil())
        }

        Stmt::Assign { target, value } => exec_assign(target, value, env),

        Stmt::AssignMulti { targets, values } => {
            // Evaluate all RHS expressions first to support parallel
            // semantics (e.g. `a, b = b, a + b`).
            let mut evaluated = Vec::with_capacity(values.len());
            for v in values {
                evaluated.push(expr::eval(v, env)?);
            }
            for (i, target) in targets.iter().enumerate() {
                let v = evaluated.get(i).cloned().unwrap_or(Value::Nil);
                assign_target(target, v, env)?;
            }
            Ok(Flow::nil())
        }

        Stmt::Expr(e) => Ok(Flow::Normal(expr::eval(e, env)?)),

        Stmt::If {
            cond,
            then_block,
            elseifs,
            else_block,
        } => {
            if expr::eval(cond, env)?.is_truthy() {
                return exec_scoped_block(then_block, env);
            }
            for (econd, ebody) in elseifs {
                if expr::eval(econd, env)?.is_truthy() {
                    return exec_scoped_block(ebody, env);
                }
            }
            if let Some(eb) = else_block {
                return exec_scoped_block(eb, env);
            }
            Ok(Flow::nil())
        }

        Stmt::While { cond, body } => {
            while expr::eval(cond, env)?.is_truthy() {
                match exec_scoped_block(body, env)? {
                    Flow::Normal(_) | Flow::Continue => continue,
                    Flow::Break => break,
                    ret @ Flow::Return(_) => return Ok(ret),
                }
            }
            Ok(Flow::nil())
        }

        Stmt::Repeat { body, cond } => {
            // Lua semantics: the `until` condition sees locals declared in
            // the body, so condition and body must share the same scope.
            loop {
                let scope = Environment::with_parent(env.clone());
                match exec_block(body, &scope)? {
                    Flow::Normal(_) | Flow::Continue => {}
                    Flow::Break => break,
                    ret @ Flow::Return(_) => return Ok(ret),
                }
                if expr::eval(cond, &scope)?.is_truthy() {
                    break;
                }
            }
            Ok(Flow::nil())
        }

        Stmt::ForNumeric {
            var,
            var_ty: _,
            from,
            to,
            step,
            body,
        } => exec_for_numeric(var, from, to, step.as_ref(), body, env, span),

        Stmt::ForIn { .. } => Err(unsupported("for-in", span)),

        Stmt::Return(exprs) => {
            // Saule supports multi-return at the syntax level but only the
            // first value is propagated until tuple support lands.
            let v = match exprs.as_slice() {
                [] => Value::Nil,
                [first, ..] => expr::eval(first, env)?,
            };
            Ok(Flow::Return(v))
        }

        Stmt::Break => Ok(Flow::Break),
        Stmt::Continue => Ok(Flow::Continue),

        Stmt::Throw(_) => Err(unsupported("throw", span)),
        Stmt::Try { .. } => Err(unsupported("try/catch", span)),
        Stmt::Decl(decl) => exec_decl(decl, env),
    }
}

// ─── Declarations ────────────────────────────────────────────────────────────

fn exec_decl(
    decl: &Spanned<Decl>,
    env: &std::rc::Rc<std::cell::RefCell<Environment>>,
) -> Result<Flow, RuntimeError> {
    let span = decl.span.clone();
    match &decl.value {
        Decl::Function {
            name, params, body, ..
        } => {
            let func = FunctionObject {
                name: Some(name.clone()),
                params: params.clone(),
                body: FunctionBody::Block(body.clone()),
                closure: env.clone(),
            };
            env.borrow_mut()
                .define(name.clone(), Value::Function(std::rc::Rc::new(func)));
            Ok(Flow::nil())
        }
        Decl::Class { .. } => exec_class_decl(decl, env),
        Decl::Interface { .. } => Err(unsupported("interface declaration", span)),
        Decl::Enum { .. } => Err(unsupported("enum declaration", span)),
        Decl::Import { .. } => Err(unsupported("import", span)),
    }
}

/// Materialize a `Decl::Class` into a [`ClassObject`] and install it under
/// the class's name in `env`. Method closures all capture `env` so they can
/// see the class itself (used by static calls) and other top-level names.
fn exec_class_decl(
    decl: &Spanned<Decl>,
    env: &Rc<RefCell<Environment>>,
) -> Result<Flow, RuntimeError> {
    let span = decl.span.clone();
    let Decl::Class {
        name,
        extends,
        members,
        ..
    } = &decl.value
    else {
        unreachable!("exec_class_decl called with non-class decl");
    };

    // Resolve parent class, if any. Must already exist in scope.
    let parent = if let Some(pname) = extends {
        match env.borrow().get(pname) {
            Some(Value::Class(c)) => Some(c),
            Some(other) => {
                return Err(RuntimeError::TypeError {
                    message: format!(
                        "cannot extend `{pname}`: expected a class, got `{}`",
                        other.type_name()
                    ),
                    span,
                });
            }
            None => {
                return Err(RuntimeError::Undefined {
                    name: pname.clone(),
                    span,
                });
            }
        }
    } else {
        None
    };

    let mut field_defs: Vec<FieldDef> = Vec::new();
    let mut methods: HashMap<String, Rc<FunctionObject>> = HashMap::new();
    let mut static_fields: HashMap<String, Value> = HashMap::new();
    let mut static_methods: HashMap<String, Rc<FunctionObject>> = HashMap::new();
    let mut constructor: Option<Rc<FunctionObject>> = None;

    // Scan once so we know whether the class has any way to be constructed.
    // When it doesn't, `local field = expr` declarations are promoted to
    // statics so callers can read them via `ClassName.field` (and through
    // the class-as-`self` convention used inside `static fn`s).
    let has_explicit_constructor = members
        .iter()
        .any(|m| matches!(&m.value, ClassMember::Constructor { .. }));
    let has_init_method = members.iter().any(|m| match &m.value {
        ClassMember::Method(meth) => meth.name == "init" && !meth.is_static,
        _ => false,
    });
    let has_constructor = has_explicit_constructor || has_init_method;

    for member in members {
        match &member.value {
            ClassMember::Field {
                is_static,
                name: fname,
                default,
                ..
            } => {
                // Promote `local field = expr` to a static when there's no
                // constructor — otherwise we'd never be able to read it.
                let treat_as_static =
                    *is_static || (!has_constructor && default.is_some());
                if treat_as_static {
                    // Static defaults are evaluated once, at class
                    // declaration time, in the enclosing scope.
                    let value = match default {
                        Some(e) => expr::eval(e, env)?,
                        None => Value::Nil,
                    };
                    static_fields.insert(fname.clone(), value);
                } else {
                    field_defs.push(FieldDef {
                        name: fname.clone(),
                        default: default.clone(),
                    });
                }
            }
            ClassMember::Constructor { params, body } => {
                constructor = Some(Rc::new(make_function(
                    Some(format!("{name}.constructor")),
                    params.clone(),
                    body.clone(),
                    env,
                )));
            }
            ClassMember::Method(m) => {
                let func = Rc::new(make_function(
                    Some(format!("{name}.{}", m.name)),
                    m.params.clone(),
                    m.body.clone(),
                    env,
                ));
                if m.is_static {
                    static_methods.insert(m.name.clone(), func);
                } else if m.name == "init" && !has_explicit_constructor {
                    // `init` is Saule's preferred constructor spelling. Only
                    // promote when there's no explicit `constructor` to
                    // avoid silently shadowing it.
                    constructor = Some(func);
                } else {
                    methods.insert(m.name.clone(), func);
                }
            }
        }
    }

    let class = Rc::new(ClassObject {
        name: name.clone(),
        parent,
        field_defs,
        methods,
        static_fields: RefCell::new(static_fields),
        static_methods,
        constructor,
    });

    env.borrow_mut()
        .define(name.clone(), Value::Class(class));
    Ok(Flow::nil())
}

fn make_function(
    name: Option<String>,
    params: Vec<Param>,
    body: Vec<Spanned<Stmt>>,
    closure: &Rc<RefCell<Environment>>,
) -> FunctionObject {
    let _ = closure; // silence unused if we change capture
    FunctionObject {
        name,
        params,
        body: FunctionBody::Block(body),
        closure: closure.clone(),
    }
}

// ─── Assignment ──────────────────────────────────────────────────────────────

fn exec_assign(
    target: &Spanned<Expr>,
    value: &Spanned<Expr>,
    env: &Rc<RefCell<Environment>>,
) -> Result<Flow, RuntimeError> {
    let v = expr::eval(value, env)?;
    assign_target(target, v, env)
}

fn assign_target(
    target: &Spanned<Expr>,
    v: Value,
    env: &Rc<RefCell<Environment>>,
) -> Result<Flow, RuntimeError> {
    match &target.value {
        Expr::Ident(name) => {
            if env.borrow_mut().assign(name, v) {
                Ok(Flow::nil())
            } else {
                Err(RuntimeError::AssignUndeclared {
                    name: name.clone(),
                    span: target.span.clone(),
                })
            }
        }
        // `obj.field = v` / `Class.static = v`
        Expr::Member { obj, name } => {
            let receiver = expr::eval(obj, env)?;
            assign_member(&receiver, name, v, target.span.clone())
        }
        // Index assignment waits for the tables phase.
        _ => Err(RuntimeError::InvalidAssignTarget {
            span: target.span.clone(),
        }),
    }
}

fn assign_member(
    receiver: &Value,
    name: &str,
    value: Value,
    span: std::ops::Range<usize>,
) -> Result<Flow, RuntimeError> {
    match receiver {
        Value::Instance(inst) => {
            inst.borrow_mut().fields.insert(name.to_string(), value);
            Ok(Flow::nil())
        }
        Value::Class(class) => {
            // Walk the chain — `Child.staticField = …` should update the
            // declaring class so the change is visible to every sibling.
            if set_static_in_chain(class, name, value.clone()) {
                Ok(Flow::nil())
            } else {
                // Define a fresh static on the most-derived class.
                class
                    .static_fields
                    .borrow_mut()
                    .insert(name.to_string(), value);
                Ok(Flow::nil())
            }
        }
        other => Err(RuntimeError::TypeError {
            message: format!(
                "cannot assign field `{name}` on value of type `{}`",
                other.type_name()
            ),
            span,
        }),
    }
}

fn set_static_in_chain(class: &Rc<crate::value::ClassObject>, name: &str, value: Value) -> bool {
    if class.static_fields.borrow().contains_key(name) {
        class
            .static_fields
            .borrow_mut()
            .insert(name.to_string(), value);
        return true;
    }
    if let Some(parent) = &class.parent {
        return set_static_in_chain(parent, name, value);
    }
    false
}

// ─── Numeric for ─────────────────────────────────────────────────────────────

fn exec_for_numeric(
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
        // Default step matches the loop's numeric type.
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
                "numeric `for` requires matching `integer` or `float` bounds (got `{}`, `{}`, `{}`)",
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
    let mut i = from;
    while (step > 0 && i <= to) || (step < 0 && i >= to) {
        let scope = Environment::with_parent(parent.clone());
        scope.borrow_mut().define(var.to_string(), Value::Int(i));
        match exec_block(body, &scope)? {
            Flow::Normal(_) | Flow::Continue => {}
            Flow::Break => return Ok(Flow::nil()),
            ret @ Flow::Return(_) => return Ok(ret),
        }
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
    let mut i = from;
    while (step > 0.0 && i <= to) || (step < 0.0 && i >= to) {
        let scope = Environment::with_parent(parent.clone());
        scope.borrow_mut().define(var.to_string(), Value::Float(i));
        match exec_block(body, &scope)? {
            Flow::Normal(_) | Flow::Continue => {}
            Flow::Break => return Ok(Flow::nil()),
            ret @ Flow::Return(_) => return Ok(ret),
        }
        i += step;
    }
    Ok(Flow::nil())
}
