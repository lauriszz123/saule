//! Walking expressions, call arguments, and the pattern grammar
//! (including the bindings a pattern introduces).

use crate::refs::Symbol;
use crate::refs::util::{
    LocalBind, contains, inferred_type_of, locate_word_in, member_name_span, strip_nullable,
};
use saule_ast::{CallArg, Expr, LambdaBody, MatchBody, Pattern, Spanned, TableEntry, Type};
use saule_semantic::{lookup_method, super_init_target, with_classes, with_enums, with_interfaces};

use super::*;

impl<'a> ResolveCx<'a> {
    pub(crate) fn visit_expr(&mut self, e: &Spanned<Expr>) {
        if !contains(&e.span, self.offset) {
            return;
        }
        match &e.value {
            Expr::Cast { value, .. } => self.visit_expr(value),
            Expr::Ident(name) => {
                if let Some(local) = self.lookup_local(name) {
                    self.record(
                        e.span.clone(),
                        Symbol::Local {
                            name: name.clone(),
                            def_span: local.def_span.clone(),
                        },
                    );
                } else if with_classes(|r| r.contains_key(name)) {
                    self.record(e.span.clone(), Symbol::Class(name.clone()));
                } else if with_interfaces(|r| r.contains_key(name)) {
                    self.record(e.span.clone(), Symbol::Interface(name.clone()));
                } else if with_enums(|r| r.contains_key(name)) {
                    self.record(e.span.clone(), Symbol::Enum(name.clone()));
                } else if !saule_typeck::sigs::is_module(name)
                    && !saule_typeck::sigs::is_value_type(name)
                    && saule_typeck::sigs::lookup(name).is_none()
                {
                    // Unrecognised — assume free function declared in
                    // this workspace.
                    self.record(e.span.clone(), Symbol::Function(name.clone()));
                }
            }
            Expr::Self_ => {
                if let Some(class) = self.enclosing_class.clone() {
                    self.record(e.span.clone(), Symbol::Class(class));
                }
            }
            Expr::Member { obj, name } | Expr::SafeMember { obj, name } => {
                if let Some(span) = member_name_span(self.source, obj.span.end, e.span.end, name)
                    && contains(&span, self.offset)
                {
                    // `self.super(...)` names no member of the enclosing
                    // class — it delegates to the parent constructor, so
                    // point goto-definition at that `init`.
                    if name == "super"
                        && matches!(obj.value, Expr::Self_)
                        && let Some(class) = &self.enclosing_class
                        && let Some((owner, _)) = super_init_target(class)
                    {
                        self.record(
                            span,
                            Symbol::Method {
                                class: owner,
                                name: "init".to_string(),
                            },
                        );
                        return;
                    }
                    if let Some(class) = self.receiver_class(&obj.value) {
                        // Enum.Variant access (no payload) — surface as
                        // a variant reference rather than a field.
                        if with_enums(|r| {
                            r.get(&class)
                                .map(|info| info.variants.contains_key(name))
                                .unwrap_or(false)
                        }) {
                            self.record(
                                span,
                                Symbol::EnumVariant {
                                    enum_name: class,
                                    variant: name.clone(),
                                },
                            );
                            return;
                        }
                        // Static method access through a class name —
                        // treat as a method reference.
                        if lookup_method(&class, name).is_some() {
                            self.record(
                                span,
                                Symbol::Method {
                                    class,
                                    name: name.clone(),
                                },
                            );
                            return;
                        }
                        self.record(
                            span,
                            Symbol::Field {
                                class,
                                name: name.clone(),
                            },
                        );
                        return;
                    }
                    self.record(
                        span,
                        Symbol::Field {
                            class: String::new(),
                            name: name.clone(),
                        },
                    );
                    return;
                }
                self.visit_expr(obj);
            }
            Expr::Index { obj, index } => {
                self.visit_expr(obj);
                self.visit_expr(index);
            }
            Expr::Call { callee, args } => {
                self.visit_expr(callee);
                for a in args {
                    self.visit_call_arg(a);
                }
            }
            Expr::MethodCall { obj, method, args } => {
                if let Some(span) = member_name_span(self.source, obj.span.end, e.span.end, method)
                    && contains(&span, self.offset)
                {
                    if let Some(class) = self.receiver_class(&obj.value) {
                        self.record(
                            span,
                            Symbol::Method {
                                class,
                                name: method.clone(),
                            },
                        );
                    }
                    return;
                }
                self.visit_expr(obj);
                for a in args {
                    self.visit_call_arg(a);
                }
            }
            Expr::Unary { rhs, .. } => self.visit_expr(rhs),
            Expr::Binary { lhs, rhs, .. } => {
                self.visit_expr(lhs);
                self.visit_expr(rhs);
            }
            Expr::ForceUnwrap(inner) => self.visit_expr(inner),
            Expr::Table(entries) => {
                for entry in entries {
                    match entry {
                        TableEntry::Positional(v) => self.visit_expr(v),
                        TableEntry::Field { key, value } => {
                            self.visit_expr(key);
                            self.visit_expr(value);
                        }
                    }
                }
            }
            Expr::Lambda { params, body, .. } => {
                let params_clone = params.clone();
                self.enter_function(&params_clone, |this| match body {
                    LambdaBody::Expr(b) => this.visit_expr(b),
                    LambdaBody::Block(b) => {
                        for s in b.iter() {
                            this.visit_stmt(s);
                        }
                    }
                });
            }
            Expr::Match { scrutinee, arms } => {
                self.visit_expr(scrutinee);
                let scrut_ty =
                    inferred_type_of(&scrutinee.value, &self.locals, &self.enclosing_class);
                for arm in arms {
                    let saved = self.locals.len();
                    self.bind_pattern(&arm.pattern, scrut_ty.as_ref());
                    self.visit_pattern(&arm.pattern);
                    if let Some(g) = &arm.guard {
                        self.visit_expr(g);
                    }
                    match &arm.body {
                        MatchBody::Expr(e) => self.visit_expr(e),
                        MatchBody::Block(b) => {
                            for s in b {
                                self.visit_stmt(s);
                            }
                        }
                    }
                    self.locals.truncate(saved);
                }
            }
            Expr::Pipe { source, stages } => {
                self.visit_expr(source);
                for st in stages {
                    for a in &st.args {
                        self.visit_call_arg(a);
                    }
                }
            }
            Expr::Int(_) | Expr::Float(_) | Expr::Bool(_) | Expr::Str(_) | Expr::Nil => {}
        }
    }

    pub(crate) fn visit_call_arg(&mut self, a: &CallArg) {
        match a {
            CallArg::Positional(e) => self.visit_expr(e),
            CallArg::Named { value, .. } => self.visit_expr(value),
        }
    }

    pub(crate) fn visit_pattern(&mut self, p: &Spanned<Pattern>) {
        if !contains(&p.span, self.offset) {
            return;
        }
        match &p.value {
            Pattern::Variant {
                enum_name,
                variant,
                fields,
            } => {
                // Enum name and variant name appear in source as
                // `Enum.Variant(...)`. Locate each within the pattern
                // span; the enum name comes first, then the variant
                // after the dot.
                if let Some(espan) = locate_word_in(self.source, &p.span, enum_name)
                    && contains(&espan, self.offset)
                {
                    self.record(espan, Symbol::Enum(enum_name.clone()));
                    return;
                }
                let after_enum = p.span.start + enum_name.len() + 1; // dot
                let lookup_range = after_enum..p.span.end;
                if let Some(vspan) = locate_word_in(self.source, &lookup_range, variant)
                    && contains(&vspan, self.offset)
                {
                    self.record(
                        vspan,
                        Symbol::EnumVariant {
                            enum_name: enum_name.clone(),
                            variant: variant.clone(),
                        },
                    );
                    return;
                }
                for f in fields {
                    self.visit_pattern(f);
                }
            }
            Pattern::Tuple(parts) => {
                for f in parts {
                    self.visit_pattern(f);
                }
            }
            Pattern::Bind(name) => {
                if let Some(span) = locate_word_in(self.source, &p.span, name)
                    && contains(&span, self.offset)
                {
                    self.record(
                        span.clone(),
                        Symbol::Local {
                            name: name.clone(),
                            def_span: span,
                        },
                    );
                }
            }
            _ => {}
        }
    }

    pub(crate) fn bind_pattern(&mut self, pat: &Spanned<Pattern>, scrut_ty: Option<&Type>) {
        match &pat.value {
            Pattern::Bind(name) => {
                let ty = scrut_ty
                    .cloned()
                    .map(strip_nullable)
                    .unwrap_or_else(|| Type::Named("any".into()));
                let span = locate_word_in(self.source, &pat.span, name)
                    .unwrap_or_else(|| pat.span.clone());
                self.locals.push(LocalBind {
                    name: name.clone(),
                    def_span: span,
                    ty,
                });
            }
            Pattern::Variant { fields, .. } | Pattern::Tuple(fields) => {
                for sub in fields {
                    self.bind_pattern(sub, None);
                }
            }
            _ => {}
        }
    }
}
