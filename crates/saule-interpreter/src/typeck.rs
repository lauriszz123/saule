//! Static checks performed between parsing and evaluation.
//!
//! This is the very first slice of what will eventually become a full
//! type-checking pass. Today it enforces a single rule:
//!
//! > Every non-nullable instance field of a class with a constructor must be
//! > assigned `self.field = ...` somewhere inside that constructor's body.
//!
//! Fields with defaults are exempt (the default initializes them), and
//! nullable fields (`name: string?`) are exempt by design.
//!
//! The walk is intentionally coarse: any `self.field = ...` anywhere inside
//! the body — even inside an `if` or loop — counts as initialized. This
//! catches the common "I forgot to assign it" bug without pretending to do
//! sound flow analysis (the future checker will).

use std::ops::Range;

use miette::Diagnostic;
use saule_ast::{ClassMember, Decl, Expr, Method, Module, Spanned, Stmt, Type};
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
}

fn to_source_span(r: Range<usize>) -> miette::SourceSpan {
    (r.start, r.end.saturating_sub(r.start)).into()
}

/// Run the static checks on a parsed module. Returns *all* errors found so
/// the user sees everything in one pass.
pub fn check(module: &Module) -> Vec<TypeCheckError> {
    let mut errors = Vec::new();
    for stmt in &module.stmts {
        check_stmt(&stmt.value, &mut errors);
    }
    errors
}

fn check_stmt(stmt: &Stmt, errors: &mut Vec<TypeCheckError>) {
    if let Stmt::Decl(decl) = stmt {
        check_decl(&decl.value, errors);
    }
}

fn check_decl(decl: &Decl, errors: &mut Vec<TypeCheckError>) {
    if let Decl::Class {
        name: class_name,
        members,
        ..
    } = decl
    {
        check_class(class_name, members, errors);
    }
}

/// True if the type forms allow `nil` to inhabit it.
fn is_nullable(ty: &Type) -> bool {
    match ty {
        Type::Nullable(_) => true,
        Type::Named(n) => n == "nil",
        _ => false,
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
    let body = match ctor_body {
        Some(b) => b,
        // No constructor → fields are either static (initialized at decl
        // time) or untouched; nothing to verify here.
        None => return,
    };

    // Collect the set of `self.X` targets that the body assigns to.
    let mut assigned: Vec<String> = Vec::new();
    for s in body {
        collect_self_assignments(&s.value, &mut assigned);
    }

    // Every non-static, non-nullable instance field without a default must
    // appear in `assigned`.
    for m in members {
        if let ClassMember::Field {
            is_static: false,
            name,
            ty,
            default: None,
        } = &m.value
        {
            if is_nullable(ty) {
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
