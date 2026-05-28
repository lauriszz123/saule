//! Definite-assignment check for class fields.
//!
//! Every non-nullable instance field without a default must be assigned
//! `self.field = ...` somewhere inside the class's `init` constructor.
//! Defaults and nullable types are exempt.

use saule_ast::{ClassMember, Expr, Method, Spanned, Stmt, Type};

use crate::error::SemanticError;
use crate::to_source_span;

pub(crate) fn check_class(
    class_name: &str,
    members: &[Spanned<ClassMember>],
    errors: &mut Vec<SemanticError>,
) {
    // Locate the constructor body: the non-static `fn init` method.
    let mut ctor_body: Option<&Vec<Spanned<Stmt>>> = None;
    for m in members {
        if let ClassMember::Method(Method {
            name,
            is_static: false,
            body,
            ..
        }) = &m.value
            && name == "init"
        {
            ctor_body = Some(body);
        }
    }

    let Some(body) = ctor_body else { return };

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
                errors.push(SemanticError::FieldNotInitialized {
                    class: class_name.to_string(),
                    field: name.clone(),
                    span: to_source_span(m.span.clone()),
                });
            }
        }
    }
}

fn is_nullable(ty: &Type) -> bool {
    matches!(ty, Type::Nullable(_))
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

