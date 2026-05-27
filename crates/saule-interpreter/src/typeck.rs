//! Static checks performed between parsing and evaluation.
//!
//! Two flavours of checks live here:
//!
//! 1. **Field initialization** — every non-nullable instance field of a class
//!    with a constructor must be assigned `self.field = ...` somewhere inside
//!    that constructor's body. Fields with defaults are exempt; nullable
//!    fields (`name: string?`) are exempt by design.
//!
//! 2. **Null safety** — values whose static type is nullable cannot be
//!    silently assigned to non-nullable bindings, and members cannot be read
//!    off a nullable receiver without `?.` / `!` / a prior nil-guard. A
//!    lightweight flow-narrowing pass treats `if x != nil then ... end` as
//!    proving `x` is non-nullable inside the then-block (and symmetrically
//!    for `if x == nil ... else ...`).
//!
//! The expression type inference here is intentionally partial: when the
//! checker can't prove a type (e.g. calls, member reads on unknown classes),
//! it returns `None` and conservatively skips the check rather than producing
//! a false positive.

use std::collections::HashMap;
use std::ops::Range;

use miette::Diagnostic;
use saule_ast::{
    BinOp, CallArg, ClassMember, Decl, Expr, LambdaBody, Method, Module, Param, Spanned, Stmt,
    Type,
};
use thiserror::Error;

/// One diagnostic produced by [`check`]. Carries a `miette` source span so
/// the CLI can render it with the offending snippet underlined.
#[derive(Debug, Error, Diagnostic)]
pub enum TypeCheckError {
    #[error("field `{field}` of class `{class}` is never initialized")]
    #[diagnostic(help(
        "assign `self.{field} = ...` in `init`, give the field a default value, or mark it nullable with `?`"
    ))]
    FieldNotInitialized {
        class: String,
        field: String,
        #[label("declared here")]
        span: miette::SourceSpan,
    },

    #[error("cannot assign `nil` to non-nullable type `{ty}`")]
    #[diagnostic(help(
        "mark the type nullable with `?` (e.g. `{ty}?`) or initialize it with a non-nil value"
    ))]
    NilToNonNullable {
        ty: String,
        #[label("`nil` not allowed here")]
        span: miette::SourceSpan,
    },

    #[error("cannot assign a nullable value of type `{from}` to non-nullable type `{to}`")]
    #[diagnostic(help(
        "guard with `if x != nil then ... end`, use `??` to provide a fallback, or force-unwrap with `!`"
    ))]
    NullableToNonNullable {
        from: String,
        to: String,
        #[label("this expression may be `nil`")]
        span: miette::SourceSpan,
    },

    #[error("cannot access `{member}` on nullable type `{ty}`")]
    #[diagnostic(help(
        "use `?.` for safe access, `!` to force-unwrap, or guard with `if x != nil then ... end`"
    ))]
    NullableMemberAccess {
        ty: String,
        member: String,
        #[label("receiver may be `nil`")]
        span: miette::SourceSpan,
    },

    #[error("default value for parameter `{param}` is incompatible with declared type `{ty}`")]
    #[diagnostic(help(
        "the default expression must produce a value of type `{ty}`"
    ))]
    DefaultParamTypeMismatch {
        param: String,
        ty: String,
        #[label("default here")]
        span: miette::SourceSpan,
    },

    #[error("return value is incompatible with declared return type `{ty}`")]
    #[diagnostic(help("this function must return a `{ty}`"))]
    WrongReturnType {
        ty: String,
        #[label("returned here")]
        span: miette::SourceSpan,
    },

    #[error("cannot access private member `{member}` of class `{class}` from outside the class")]
    #[diagnostic(help(
        "`local` fields and methods are only accessible from within `{class}`"
    ))]
    PrivateMemberAccess {
        class: String,
        member: String,
        #[label("private")]
        span: miette::SourceSpan,
    },

    #[error("table value of type `{found}` is incompatible with declared element type `{expected}`")]
    #[diagnostic(help(
        "every value stored in this table must be a `{expected}`"
    ))]
    TableElementTypeMismatch {
        expected: String,
        found: String,
        #[label("wrong value type")]
        span: miette::SourceSpan,
    },

    #[error("table key of type `{found}` is incompatible with declared key type `{expected}`")]
    #[diagnostic(help(
        "this table is declared with key type `{expected}` — pass an index of that type"
    ))]
    TableKeyTypeMismatch {
        expected: String,
        found: String,
        #[label("wrong key type")]
        span: miette::SourceSpan,
    },

    #[error("cannot initialise `table<{key}, {value}>` with an array-style literal")]
    #[diagnostic(help(
        "array-style `{{ ... }}` literals can only fill `table<T>` (integer-keyed); start from `{{}}` and assign by key instead"
    ))]
    TableArrayLiteralForMap {
        key: String,
        value: String,
        #[label("array literal not allowed for a map-typed table")]
        span: miette::SourceSpan,
    },

    #[error("cannot iterate over a `{class}` — it does not implement `Iterable` or `Iterable2`")]
    #[diagnostic(help(
        "add `implements Iterable<T>` (or `Iterable2<K, V>`) to `{class}` and define `fn iter() -> fn(): T?` returning a step closure"
    ))]
    NotIterable {
        class: String,
        #[label("class is not iterable")]
        span: miette::SourceSpan,
    },

    #[error("argument {arg} of `{callee}` expects `{expected}`, got `{found}`")]
    #[diagnostic(help(
        "pass a value of type `{expected}` here — check the signature of `{callee}`"
    ))]
    NativeArgTypeMismatch {
        callee: String,
        arg: usize,
        expected: String,
        found: String,
        #[label("wrong argument type")]
        span: miette::SourceSpan,
    },

    #[error("`{callee}` expects {expected} argument(s), got {found}")]
    #[diagnostic(help("check the signature of `{callee}`"))]
    NativeArity {
        callee: String,
        expected: usize,
        found: usize,
        #[label("wrong number of arguments")]
        span: miette::SourceSpan,
    },
}

fn to_source_span(r: Range<usize>) -> miette::SourceSpan {
    (r.start, r.end.saturating_sub(r.start)).into()
}

// ──────────────────────────────────────────────────────────────────────────────
// Scope: tracks the static types of `local` bindings in lexical scope.
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Default, Clone)]
struct Scope {
    vars: HashMap<String, Type>,
}

impl Scope {
    fn lookup(&self, name: &str) -> Option<&Type> {
        self.vars.get(name)
    }

    fn bind(&mut self, name: String, ty: Type) {
        self.vars.insert(name, ty);
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Public entry point.
// ──────────────────────────────────────────────────────────────────────────────

/// Class info collected by a pre-pass so member-access checks can consult
/// member visibility, parent classes, etc.
#[derive(Default, Clone)]
struct ClassInfo {
    parent: Option<String>,
    /// Interfaces declared on the class (`class C implements A, B`).
    implements: Vec<String>,
    /// member name -> is_private
    members: HashMap<String, bool>,
}

type ClassRegistry = HashMap<String, ClassInfo>;

thread_local! {
    static CLASSES: std::cell::RefCell<ClassRegistry> = std::cell::RefCell::new(HashMap::new());
    static CURRENT_CLASS: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

fn with_classes<R>(f: impl FnOnce(&ClassRegistry) -> R) -> R {
    CLASSES.with(|c| f(&c.borrow()))
}

fn current_class() -> Option<String> {
    CURRENT_CLASS.with(|c| c.borrow().clone())
}

fn set_current_class(name: Option<String>) -> Option<String> {
    CURRENT_CLASS.with(|c| std::mem::replace(&mut *c.borrow_mut(), name))
}

/// Look up `member` on `class` (walking the parent chain). Returns
/// `Some((owning_class, is_private))` if found.
fn lookup_member(class: &str, member: &str) -> Option<(String, bool)> {
    with_classes(|reg| {
        let mut cur = Some(class.to_string());
        while let Some(name) = cur {
            if let Some(info) = reg.get(&name) {
                if let Some(&priv_) = info.members.get(member) {
                    return Some((name, priv_));
                }
                cur = info.parent.clone();
            } else {
                return None;
            }
        }
        None
    })
}

/// Does the class (or any ancestor) declare it implements `Iterable` or
/// `Iterable2`? Used by the `for ... in` static check.
fn class_implements_iterable(class: &str) -> bool {
    with_classes(|reg| {
        let mut cur = Some(class.to_string());
        while let Some(name) = cur {
            let Some(info) = reg.get(&name) else {
                return false;
            };
            if info
                .implements
                .iter()
                .any(|i| i == "Iterable" || i == "Iterable2")
            {
                return true;
            }
            cur = info.parent.clone();
        }
        false
    })
}

fn build_registry(module: &Module) -> ClassRegistry {    let mut reg = ClassRegistry::new();
    for stmt in &module.stmts {
        if let Stmt::Decl(d) = &stmt.value
            && let Decl::Class {
                name,
                extends,
                implements,
                members,
                ..
            } = &d.value
        {
            let mut info = ClassInfo {
                parent: extends.clone(),
                implements: implements.clone(),
                members: HashMap::new(),
            };
            for m in members {
                match &m.value {
                    ClassMember::Field {
                        name, is_private, ..
                    } => {
                        info.members.insert(name.clone(), *is_private);
                    }
                    ClassMember::Method(meth) => {
                        info.members.insert(meth.name.clone(), meth.is_private);
                    }
                }
            }
            reg.insert(name.clone(), info);
        }
    }
    reg
}

/// Run the static checks on a parsed module. Returns *all* errors found so
/// the user sees everything in one pass.
pub fn check(module: &Module) -> Vec<TypeCheckError> {
    let reg = build_registry(module);
    CLASSES.with(|c| *c.borrow_mut() = reg);
    let _restore = set_current_class(None);
    let mut errors = Vec::new();
    let mut scope = Scope::default();
    for stmt in &module.stmts {
        check_stmt(&stmt.value, &mut scope, &mut errors);
    }
    CLASSES.with(|c| c.borrow_mut().clear());
    errors
}

// ──────────────────────────────────────────────────────────────────────────────
// Statement walker. Threads a `Scope` so we know the static type of every
// `local` we've seen on the current path, and so narrowing in `if`/`else` can
// override types for the duration of a sub-block.
// ──────────────────────────────────────────────────────────────────────────────

fn check_stmt(stmt: &Stmt, scope: &mut Scope, errors: &mut Vec<TypeCheckError>) {
    match stmt {
        Stmt::Decl(decl) => check_decl(&decl.value, errors),

        Stmt::Local { name, ty, value } => {
            if let (Some(ty), Some(v)) = (ty, value) {
                check_expr(v, scope, errors);
                check_assignment_compat(ty, v, scope, errors);
                scope.bind(name.clone(), ty.clone());
            } else if let Some(v) = value {
                check_expr(v, scope, errors);
                if let Some(t) = infer(v, scope) {
                    scope.bind(name.clone(), t);
                }
            } else if let Some(ty) = ty {
                scope.bind(name.clone(), ty.clone());
            }
        }

        Stmt::LocalMulti { names, values } => {
            for v in values {
                check_expr(v, scope, errors);
            }
            for (i, (name, ty_opt)) in names.iter().enumerate() {
                if let (Some(ty), Some(v)) = (ty_opt, values.get(i)) {
                    check_assignment_compat(ty, v, scope, errors);
                }
                if let Some(ty) = ty_opt {
                    scope.bind(name.clone(), ty.clone());
                } else if let Some(v) = values.get(i)
                    && let Some(t) = infer(v, scope)
                {
                    scope.bind(name.clone(), t);
                }
            }
        }

        Stmt::Assign { target, value } => {
            check_expr(target, scope, errors);
            check_expr(value, scope, errors);
            if let Expr::Ident(n) = &target.value
                && let Some(ty) = scope.lookup(n).cloned()
            {
                check_assignment_compat(&ty, value, scope, errors);
            }
            // `t[k] = v` — enforce the table's static key/value types.
            if let Expr::Index { obj, index } = &target.value
                && let Some(Type::Table { key, value: elem_ty }) = infer(obj, scope)
            {
                let key_ty = key
                    .as_deref()
                    .cloned()
                    .unwrap_or_else(|| Type::Named("integer".into()));
                check_table_key_compat(&key_ty, index, scope, errors);
                check_element_compat(&elem_ty, value, scope, errors);
            }
        }

        Stmt::AssignMulti { targets, values } => {
            for t in targets {
                check_expr(t, scope, errors);
            }
            for v in values {
                check_expr(v, scope, errors);
            }
            for (i, target) in targets.iter().enumerate() {
                if let Expr::Ident(n) = &target.value
                    && let (Some(ty), Some(v)) = (scope.lookup(n).cloned(), values.get(i))
                {
                    check_assignment_compat(&ty, v, scope, errors);
                }
            }
        }

        Stmt::Expr(e) => check_expr(e, scope, errors),

        Stmt::If {
            cond,
            then_block,
            elseifs,
            else_block,
        } => {
            check_expr(cond, scope, errors);

            // Branch the scope so narrowing in the then-block doesn't leak.
            let mut then_scope = scope.clone();
            narrow_truthy(cond, &mut then_scope);
            for s in then_block {
                check_stmt(&s.value, &mut then_scope, errors);
            }

            for (econd, ebody) in elseifs {
                check_expr(econd, scope, errors);
                let mut ei_scope = scope.clone();
                narrow_truthy(econd, &mut ei_scope);
                for s in ebody {
                    check_stmt(&s.value, &mut ei_scope, errors);
                }
            }

            if let Some(block) = else_block {
                let mut else_scope = scope.clone();
                narrow_falsy(cond, &mut else_scope);
                for s in block {
                    check_stmt(&s.value, &mut else_scope, errors);
                }
            }
        }

        Stmt::While { cond, body } | Stmt::Repeat { body, cond } => {
            check_expr(cond, scope, errors);
            let mut body_scope = scope.clone();
            narrow_truthy(cond, &mut body_scope);
            for s in body {
                check_stmt(&s.value, &mut body_scope, errors);
            }
        }

        Stmt::ForNumeric {
            var,
            var_ty,
            from,
            to,
            step,
            body,
        } => {
            check_expr(from, scope, errors);
            check_expr(to, scope, errors);
            if let Some(s) = step {
                check_expr(s, scope, errors);
            }
            let mut body_scope = scope.clone();
            let ty = var_ty.clone().unwrap_or(Type::Named("integer".into()));
            body_scope.bind(var.clone(), ty);
            for s in body {
                check_stmt(&s.value, &mut body_scope, errors);
            }
        }

        Stmt::ForIn { vars, iter, body } => {
            check_expr(iter, scope, errors);
            // If the iter expression is a known class instance, it must
            // implement `Iterable` or `Iterable2` (walking the parent chain).
            if let Some(Type::Named(class_name)) = infer(iter, scope)
                && with_classes(|reg| reg.contains_key(&class_name))
                && !class_implements_iterable(&class_name)
            {
                errors.push(TypeCheckError::NotIterable {
                    class: class_name,
                    span: to_source_span(iter.span.clone()),
                });
            }
            let mut body_scope = scope.clone();
            for (name, ty_opt) in vars {
                if let Some(ty) = ty_opt {
                    body_scope.bind(name.clone(), ty.clone());
                }
            }
            for s in body {
                check_stmt(&s.value, &mut body_scope, errors);
            }
        }

        Stmt::Return(values) => {
            for v in values {
                check_expr(v, scope, errors);
            }
        }

        Stmt::Throw(e) => check_expr(e, scope, errors),

        Stmt::Try {
            body,
            catch_var,
            catch_ty,
            catch_body,
        } => {
            let mut body_scope = scope.clone();
            for s in body {
                check_stmt(&s.value, &mut body_scope, errors);
            }
            let mut catch_scope = scope.clone();
            catch_scope.bind(catch_var.clone(), catch_ty.clone());
            for s in catch_body {
                check_stmt(&s.value, &mut catch_scope, errors);
            }
        }

        Stmt::Break | Stmt::Continue => {}
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Declaration walker.
// ──────────────────────────────────────────────────────────────────────────────

fn check_decl(decl: &Decl, errors: &mut Vec<TypeCheckError>) {
    match decl {
        Decl::Class {
            name: class_name,
            members,
            ..
        } => check_class(class_name, members, errors),
        Decl::Function {
            params,
            return_ty,
            body,
            ..
        } => {
            let mut scope = Scope::default();
            check_default_params(params, &scope, errors);
            seed_params(&mut scope, params);
            for s in body {
                check_stmt(&s.value, &mut scope, errors);
            }
            if let Some(rt) = return_ty {
                check_returns(body, rt, &scope, errors);
            }
        }
        _ => {}
    }
}

fn seed_params(scope: &mut Scope, params: &[Param]) {
    for p in params {
        scope.bind(p.name.clone(), p.ty.clone());
    }
}

/// Check each parameter default against the declared parameter type.
fn check_default_params(params: &[Param], scope: &Scope, errors: &mut Vec<TypeCheckError>) {
    for p in params {
        if let Some(d) = &p.default
            && !is_assignment_compatible(&p.ty, d, scope)
        {
            errors.push(TypeCheckError::DefaultParamTypeMismatch {
                param: p.name.clone(),
                ty: type_to_string(&p.ty),
                span: to_source_span(d.span.clone()),
            });
        }
    }
}

/// Walk a function/method body looking for `return v` statements whose first
/// value can be proved incompatible with `return_ty`.
fn check_returns(
    body: &[Spanned<Stmt>],
    return_ty: &Type,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    for s in body {
        walk_returns(&s.value, return_ty, scope, errors);
    }
}

fn walk_returns(
    stmt: &Stmt,
    return_ty: &Type,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    match stmt {
        Stmt::Return(values) => {
            if let Some(v) = values.first()
                && !is_assignment_compatible(return_ty, v, scope)
            {
                errors.push(TypeCheckError::WrongReturnType {
                    ty: type_to_string(return_ty),
                    span: to_source_span(v.span.clone()),
                });
            }
        }
        Stmt::If {
            then_block,
            elseifs,
            else_block,
            ..
        } => {
            for s in then_block {
                walk_returns(&s.value, return_ty, scope, errors);
            }
            for (_, b) in elseifs {
                for s in b {
                    walk_returns(&s.value, return_ty, scope, errors);
                }
            }
            if let Some(b) = else_block {
                for s in b {
                    walk_returns(&s.value, return_ty, scope, errors);
                }
            }
        }
        Stmt::While { body, .. }
        | Stmt::Repeat { body, .. }
        | Stmt::ForNumeric { body, .. }
        | Stmt::ForIn { body, .. } => {
            for s in body {
                walk_returns(&s.value, return_ty, scope, errors);
            }
        }
        Stmt::Try {
            body, catch_body, ..
        } => {
            for s in body {
                walk_returns(&s.value, return_ty, scope, errors);
            }
            for s in catch_body {
                walk_returns(&s.value, return_ty, scope, errors);
            }
        }
        _ => {}
    }
}

/// True when we can *prove* the value is incompatible-free with the target
/// type. Returns true when we can't decide (conservative: don't false-positive).
fn is_assignment_compatible(decl_ty: &Type, value: &Spanned<Expr>, scope: &Scope) -> bool {
    if is_nullable(decl_ty) {
        // Nullable target accepts anything we can express.
        return true;
    }
    if matches!(value.value, Expr::Nil) {
        return false;
    }
    let Some(value_ty) = infer(value, scope) else {
        return true;
    };
    if is_nullable(&value_ty) {
        return false;
    }
    match (decl_ty, &value_ty) {
        (Type::Named(a), Type::Named(b)) => {
            if a == b || a == "any" || b == "any" {
                true
            } else {
                // Allow numeric literals in either direction only when same name.
                false
            }
        }
        _ => true,
    }
}

fn check_class(
    class_name: &str,
    members: &[Spanned<ClassMember>],
    errors: &mut Vec<TypeCheckError>,
) {
    // Locate the constructor body: the non-static `fn init` method.
    let mut ctor_body: Option<&Vec<Spanned<Stmt>>> = None;
    for m in members {
        match &m.value {
            ClassMember::Method(Method {
                name,
                is_static: false,
                body,
                ..
            }) if name == "init" => {
                ctor_body = Some(body);
            }
            _ => {}
        }
    }

    // Validate field defaults: `local x: string = nil` is an error regardless
    // of whether a constructor exists or the field is static.
    for m in members {
        if let ClassMember::Field {
            ty,
            default: Some(default_expr),
            ..
        } = &m.value
        {
            let scope = Scope::default();
            check_assignment_compat(ty, default_expr, &scope, errors);
        }
    }

    if let Some(body) = ctor_body {
        let mut assigned: Vec<String> = Vec::new();
        for s in body {
            collect_self_assignments(&s.value, &mut assigned);
        }
        for m in members {
            if let ClassMember::Field {
                is_static: false,
                name,
                ty,
                default,
                ..
            } = &m.value
            {
                if default.is_some() || is_nullable(ty) {
                    continue;
                }
                if !assigned.iter().any(|a| a == name) {
                    errors.push(TypeCheckError::FieldNotInitialized {
                        class: class_name.to_string(),
                        field: name.clone(),
                        span: to_source_span(m.span.clone()),
                    });
                }
            }
        }
    }

    // Walk every method body with a scope seeded from its parameters,
    // and within `CURRENT_CLASS` set so private-member checks know we're
    // *inside* `class_name`. Also validate default parameters and return types.
    let prev = set_current_class(Some(class_name.to_string()));
    for m in members {
        if let ClassMember::Method(meth) = &m.value {
            let mut scope = Scope::default();
            // `self` resolves to the class itself in `static fn` and to an
            // instance otherwise. Seed it as the class name so member-existence
            // checks on `self.foo` consult the class registry.
            scope.bind("self".to_string(), Type::Named(class_name.to_string()));
            check_default_params(&meth.params, &scope, errors);
            seed_params(&mut scope, &meth.params);
            for s in &meth.body {
                check_stmt(&s.value, &mut scope, errors);
            }
            if let Some(rt) = &meth.return_ty {
                check_returns(&meth.body, rt, &scope, errors);
            }
        }
    }
    set_current_class(prev);
}

// ──────────────────────────────────────────────────────────────────────────────
// Expression checker — walks expressions looking for `obj.member` /
// `obj.method(...)` where `obj` has a statically-known nullable type.
// ──────────────────────────────────────────────────────────────────────────────

fn check_expr(expr: &Spanned<Expr>, scope: &Scope, errors: &mut Vec<TypeCheckError>) {
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
                && let Some(sig) = crate::stdlib::sigs::lookup(&qname)
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
            for e in items {
                check_expr(e, scope, errors);
            }
        }
        Expr::Lambda { params, body, .. } => {
            let mut lscope = scope.clone();
            seed_params(&mut lscope, params);
            match body {
                LambdaBody::Expr(e) => check_expr(e, &lscope, errors),
                LambdaBody::Block(stmts) => {
                    for s in stmts {
                        check_stmt(&s.value, &mut lscope, errors);
                    }
                }
            }
        }
        _ => {}
    }
}

fn check_arg(arg: &CallArg, scope: &Scope, errors: &mut Vec<TypeCheckError>) {
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
fn check_native_args(
    callee: &str,
    sig: &crate::stdlib::sigs::NativeSig,
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
    let positional: Vec<&CallArg> = args.iter().filter(|a| matches!(a, CallArg::Positional(_))).collect();
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

fn is_any(t: &Type) -> bool {
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
/// `self.foo` is always permitted; access via any other expression is checked.
fn report_if_private(
    obj: &Spanned<Expr>,
    member: &str,
    scope: &Scope,
    errors: &mut Vec<TypeCheckError>,
) {
    // Accesses through `self` are always allowed (inside their own class).
    if matches!(obj.value, Expr::Self_) {
        return;
    }
    let Some(ty) = infer(obj, scope) else { return };
    let class_name = match strip_nullable(ty) {
        Type::Named(n) => n,
        _ => return,
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

fn check_assignment_compat(
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

    // Table-aware checks for array-style literals assigned to a typed table.
    if let (Type::Table { key, value: elem_ty }, Expr::Table(items)) = (decl_ty, &value.value) {
        // `{a, b, c}` literal cannot fill a map-typed table whose key is not
        // integer-compatible.
        if let Some(k) = key
            && !is_integer_like(k)
            && !items.is_empty()
        {
            errors.push(TypeCheckError::TableArrayLiteralForMap {
                key: type_to_string(k),
                value: type_to_string(elem_ty),
                span: to_source_span(value.span.clone()),
            });
            return;
        }
        // Each element must match the declared value type.
        for item in items {
            check_element_compat(elem_ty, item, scope, errors);
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
fn check_table_key_compat(
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

/// Element-of-table compatibility — accepts literals/`Ident`s whose inferred
/// type matches, and stays quiet otherwise (conservative).
fn check_element_compat(
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
fn types_compatible(expected: &Type, value_ty: &Type) -> bool {
    match (expected, value_ty) {
        // Same-name primitives, plus `any` on either side, plus `nil` on the
        // value side (nil is universally assignable; nullable-rejection is
        // handled separately by `NullableToNonNullable`).
        (Type::Named(a), Type::Named(b)) => {
            if a == b || a == "any" || b == "any" || b == "nil" {
                return true;
            }
            // `number` is the sentinel used in native sigs to mean
            // "integer or float" — accept either.
            if a == "number" && (b == "integer" || b == "float" || b == "number") {
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

// ──────────────────────────────────────────────────────────────────────────────
// Lightweight type inference.
//
// Returns `Some(ty)` only when we can prove the type. Anything we can't see
// through (calls, member reads on unknown classes, indexing) returns `None`,
// and callers treat that as "don't know, don't complain".
// ──────────────────────────────────────────────────────────────────────────────

fn infer(expr: &Spanned<Expr>, scope: &Scope) -> Option<Type> {
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
                && let Some(sig) = crate::stdlib::sigs::lookup(&qname)
            {
                first_or_tuple(sig.returns)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn strip_nullable(ty: Type) -> Type {
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

fn narrow_truthy(cond: &Spanned<Expr>, scope: &mut Scope) {
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

fn narrow_falsy(cond: &Spanned<Expr>, scope: &mut Scope) {
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

fn type_to_string(ty: &Type) -> String {
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

fn is_nullable(ty: &Type) -> bool {
    match ty {
        Type::Nullable(_) => true,
        Type::Named(n) => n == "nil",
        _ => false,
    }
}

/// Walk a statement collecting every `self.NAME` that appears on the LHS of
/// an `=`. Recurses through control-flow statements so an assignment inside
/// an `if` still counts.
fn collect_self_assignments(stmt: &Stmt, out: &mut Vec<String>) {
    match stmt {
        Stmt::Assign { target, .. } => {
            if let Expr::Member { obj, name } = &target.value
                && matches!(obj.value, Expr::Self_)
                && !out.iter().any(|n| n == name)
            {
                out.push(name.clone());
            }
        }
        Stmt::AssignMulti { targets, .. } => {
            for target in targets {
                if let Expr::Member { obj, name } = &target.value
                    && matches!(obj.value, Expr::Self_)
                    && !out.iter().any(|n| n == name)
                {
                    out.push(name.clone());
                }
            }
        }
        Stmt::If {
            then_block,
            elseifs,
            else_block,
            ..
        } => {
            for s in then_block {
                collect_self_assignments(&s.value, out);
            }
            for (_, block) in elseifs {
                for s in block {
                    collect_self_assignments(&s.value, out);
                }
            }
            if let Some(block) = else_block {
                for s in block {
                    collect_self_assignments(&s.value, out);
                }
            }
        }
        Stmt::While { body, .. }
        | Stmt::Repeat { body, .. }
        | Stmt::ForNumeric { body, .. }
        | Stmt::ForIn { body, .. } => {
            for s in body {
                collect_self_assignments(&s.value, out);
            }
        }
        Stmt::Try {
            body, catch_body, ..
        } => {
            for s in body {
                collect_self_assignments(&s.value, out);
            }
            for s in catch_body {
                collect_self_assignments(&s.value, out);
            }
        }
        _ => {}
    }
}
