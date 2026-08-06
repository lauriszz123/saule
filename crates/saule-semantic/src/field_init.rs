//! Definite-assignment check for class fields.
//!
//! Every non-nullable instance field without a default must be assigned
//! `self.field = ...` somewhere inside the class's `init` constructor. A
//! class with no `init` at all has nowhere to do that, so every such field
//! is reported. Defaults and nullable types are exempt.
//!
//! `static local` fields are stricter: nothing runs before the first read
//! of a static, so there is no constructor-shaped escape hatch. A
//! non-nullable static must carry its value in the declaration.

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

    // No `init` means no field is ever assigned — the empty set below then
    // reports every non-nullable field without a default, which is right:
    // instances of such a class would start out holding `nil`.
    let mut assigned: Vec<String> = Vec::new();
    if let Some(body) = ctor_body {
        for s in body {
            collect_self_assignments(&s.value, &mut assigned);
        }
    }

    for m in members {
        let ClassMember::Field {
            is_static,
            name,
            ty,
            default,
            ..
        } = &m.value
        else {
            continue;
        };
        if default.is_some() || is_nullable(ty) {
            continue;
        }
        if *is_static {
            errors.push(SemanticError::StaticFieldNotInitialized {
                class: class_name.to_string(),
                field: name.clone(),
                span: to_source_span(m.span.clone()),
            });
        } else if !assigned.iter().any(|a| a == name) {
            errors.push(SemanticError::FieldNotInitialized {
                class: class_name.to_string(),
                field: name.clone(),
                span: to_source_span(m.span.clone()),
            });
        }
    }
}

fn is_nullable(ty: &Type) -> bool {
    matches!(ty, Type::Nullable(_))
}

/// Walk a statement collecting every `self.NAME` that appears on the LHS of
/// an `=`. Recurses through control-flow statements so an assignment inside
/// an `if` still counts.
///
/// `Stmt::CompoundAssign` is deliberately *not* collected: `self.n += 1`
/// reads `self.n` before it writes it, so it initialises nothing and a field
/// whose only mention in `init` is a compound assignment is still uninitialised.
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
