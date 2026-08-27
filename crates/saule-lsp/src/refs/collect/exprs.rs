//! Walking expressions, call arguments, and the pattern grammar
//! (including the bindings a pattern introduces).

use crate::refs::Symbol;
use crate::refs::util::{
    LocalBind, field_owner, inferred_type_of, locate_word_in, member_name_span, method_owner,
    static_method_owner, strip_nullable,
};
use saule_ast::{CallArg, Expr, LambdaBody, MatchBody, Pattern, Spanned, TableEntry, Type};
use saule_semantic::super_init_target;

use super::*;

impl<'a> CollectCx<'a> {
    pub(crate) fn visit_expr(&mut self, e: &Spanned<Expr>) {
        match &e.value {
            // A recovery hole has no children to walk.
            Expr::Error => {}
            Expr::Cast { value, .. } => self.visit_expr(value),
            Expr::Ident(name) => match self.symbol {
                Symbol::Local {
                    name: tname,
                    def_span,
                } => {
                    if name == tname
                        && self
                            .lookup_local(name)
                            .is_some_and(|l| &l.def_span == def_span)
                    {
                        self.push(e.span.clone(), false);
                    }
                }
                Symbol::Class(t) | Symbol::Interface(t) | Symbol::Enum(t) | Symbol::Function(t)
                    if name == t && self.lookup_local(name).is_none() =>
                {
                    self.push(e.span.clone(), false);
                }
                // Bare reference to an enclosing class's static method —
                // mirrors what the cursor resolver records for `help()`
                // written inside the class that declares it.
                Symbol::Method {
                    class: tc,
                    name: tn,
                } if name == tn
                    && self.lookup_local(name).is_none()
                    && self
                        .enclosing_class
                        .as_deref()
                        .and_then(|c| static_method_owner(c, name))
                        .as_deref()
                        == Some(tc.as_str()) =>
                {
                    self.push(e.span.clone(), false);
                }
                _ => {}
            },
            Expr::Self_ => {}
            Expr::Member { obj, name } | Expr::SafeMember { obj, name } => {
                self.visit_expr(obj);
                let Some(span) = member_name_span(self.source, obj.span.end, e.span.end, name)
                else {
                    return;
                };
                // `self.super(...)` is a reference to the parent
                // constructor, not to a member named `super` — mirror
                // what the cursor resolver records for it.
                if name == "super"
                    && matches!(obj.value, Expr::Self_)
                    && let Symbol::Method {
                        class: tc,
                        name: tn,
                    } = self.symbol
                    && tn == "init"
                    && let Some(enclosing) = &self.enclosing_class
                    && let Some((owner, _)) = super_init_target(enclosing)
                    && &owner == tc
                {
                    self.push(span, false);
                    return;
                }
                let class = self.receiver_class(&obj.value);
                match self.symbol {
                    // An inherited member is keyed on the class that
                    // declares it, so the receiver's own class is walked
                    // up the chain before comparing — exactly what the
                    // cursor resolver does.
                    Symbol::Field {
                        class: tc,
                        name: tn,
                    } => {
                        if name == tn
                            && class
                                .as_deref()
                                .and_then(|c| field_owner(c, name))
                                .as_deref()
                                == Some(tc.as_str())
                        {
                            self.push(span, false);
                        }
                    }
                    Symbol::Method {
                        class: tc,
                        name: tn,
                    } => {
                        if name == tn
                            && class
                                .as_deref()
                                .and_then(|c| method_owner(c, name))
                                .as_deref()
                                == Some(tc.as_str())
                        {
                            self.push(span, false);
                        }
                    }
                    Symbol::EnumVariant { enum_name, variant }
                        if name == variant && class.as_deref() == Some(enum_name.as_str()) =>
                    {
                        self.push(span, false);
                    }
                    _ => {}
                }
            }
            Expr::Index { obj, index } => {
                self.visit_expr(obj);
                self.visit_expr(index);
            }
            Expr::Call { callee, args, .. } => {
                self.visit_expr(callee);
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
            Expr::Lambda {
                params,
                return_ty,
                body,
            } => {
                let params_clone = params.clone();
                self.enter_lambda(&params_clone, |this| {
                    for p in params {
                        this.collect_type_name_refs_in(&p.ty, &p.span);
                        if let Some(def) = &p.default {
                            this.visit_expr(def);
                        }
                    }
                    if let Some(rt) = return_ty {
                        let after = params.last().map(|p| p.span.end).unwrap_or(e.span.start);
                        this.collect_type_name_refs_in(rt, &(after..e.span.end));
                    }
                    match body {
                        LambdaBody::Expr(b) => this.visit_expr(b),
                        LambdaBody::Block(b) => {
                            for s in b.iter() {
                                this.visit_stmt(s);
                            }
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
                    // `:stage()` calls the free function `stage`.
                    if let Symbol::Function(target) = self.symbol
                        && target == &st.name
                        && let Some(span) = locate_word_in(self.source, &st.span, &st.name)
                    {
                        self.push(span, false);
                    }
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
        match &p.value {
            Pattern::Variant {
                enum_name,
                variant,
                fields,
            } => {
                match self.symbol {
                    Symbol::Enum(t) if t == enum_name => {
                        if let Some(span) = locate_word_in(self.source, &p.span, enum_name) {
                            self.push(span, false);
                        }
                    }
                    Symbol::EnumVariant {
                        enum_name: tn,
                        variant: tv,
                    } if tn == enum_name && tv == variant => {
                        let after = p.span.start + enum_name.len() + 1;
                        let lookup = after..p.span.end;
                        if let Some(span) = locate_word_in(self.source, &lookup, variant) {
                            self.push(span, false);
                        }
                    }
                    _ => {}
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
                if let Symbol::Local { name: tn, def_span } = self.symbol
                    && tn == name
                    && let Some(span) = locate_word_in(self.source, &p.span, name)
                    && &span == def_span
                {
                    self.push(span, true);
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
