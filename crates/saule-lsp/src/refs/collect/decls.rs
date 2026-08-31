//! Walking declarations: functions, classes, their members and
//! methods, and the type names in a class header.

use crate::refs::Symbol;
use crate::refs::util::{declared_name, locate_import_path, locate_word_in, locate_words_in};
use saule_ast::{ClassMember, Decl, EnumVariant, Method, Spanned};

use super::*;

impl<'a> CollectCx<'a> {
    pub(crate) fn visit_decl(&mut self, d: &Spanned<Decl>) {
        match &d.value {
            Decl::Function {
                name,
                params,
                return_ty,
                body,
                ..
            } => {
                if let Symbol::Function(target) = self.symbol
                    && target == name
                    && let Some(span) = locate_word_in(self.source, &d.span, name)
                {
                    self.push(span, true);
                }
                if let Some(rt) = return_ty {
                    let after = params.last().map(|p| p.span.end).unwrap_or(d.span.start);
                    let before = body.first().map(|s| s.span.start).unwrap_or(d.span.end);
                    self.collect_type_name_refs_in(rt, &(after..before));
                }
                self.enter_function(params, |this| {
                    for p in params {
                        this.collect_type_name_refs_in(&p.ty, &p.span);
                        if let Some(def) = &p.default {
                            this.visit_expr(def);
                        }
                    }
                    for s in body {
                        this.visit_stmt(s);
                    }
                });
            }
            Decl::Class {
                name,
                extends,
                implements,
                members,
                ..
            } => {
                if let Symbol::Class(target) = self.symbol
                    && target == name
                    && let Some(span) = locate_word_in(self.source, &d.span, name)
                {
                    self.push(span, true);
                }
                // `extends` / `implements` references
                let implemented: Vec<String> = implements.iter().map(|i| i.name.clone()).collect();
                self.collect_type_name_refs_in_header(
                    d,
                    extends.as_ref().map(|e| e.name.as_str()),
                    &implemented,
                );
                let prev = self.enclosing_class.replace(name.clone());
                for m in members {
                    self.visit_member(m);
                }
                self.enclosing_class = prev;
            }
            Decl::Interface {
                name,
                extends,
                methods,
                ..
            } => {
                if let Symbol::Interface(target) = self.symbol
                    && target == name
                    && let Some(span) = locate_word_in(self.source, &d.span, name)
                {
                    self.push(span, true);
                }
                let parents: Vec<String> = extends.iter().map(|e| e.name.clone()).collect();
                self.collect_type_name_refs_in_header(d, None, &parents);
                // Method-sig param/return type references handled via
                // a span-bounded source scan inside the decl head.
                let _ = methods;
            }
            Decl::Enum {
                name,
                variants,
                methods,
                ..
            } => {
                if let Symbol::Enum(target) = self.symbol
                    && target == name
                    && let Some(span) = locate_word_in(self.source, &d.span, name)
                {
                    self.push(span, true);
                }
                for v in variants {
                    let vname = match &v.value {
                        EnumVariant::Bare(n) | EnumVariant::Valued(n, _) => n,
                        EnumVariant::Tuple { name, .. } => name,
                    };
                    if let Symbol::EnumVariant {
                        enum_name: en,
                        variant,
                    } = self.symbol
                        && en == name
                        && variant == vname
                        && let Some(span) = locate_word_in(self.source, &v.span, vname)
                    {
                        self.push(span, true);
                    }
                    if let EnumVariant::Valued(_, e) = &v.value {
                        self.visit_expr(e);
                    }
                }
                let prev = self.enclosing_class.replace(name.clone());
                for m in methods {
                    self.visit_method(m, name);
                }
                self.enclosing_class = prev;
            }
            // A module variable renames like a local: the declaration plus
            // every bare use of the name in this file.
            Decl::Variable {
                name,
                name_span,
                ty,
                value,
                ..
            } => {
                if let Some(v) = value {
                    self.visit_expr(v);
                }
                if let Some(t) = ty {
                    let head_end = value.as_ref().map(|v| v.span.start).unwrap_or(d.span.end);
                    self.collect_type_name_refs_in(t, &(d.span.start..head_end));
                }
                if let Symbol::Local {
                    name: tname,
                    def_span: tspan,
                } = self.symbol
                    && tname == name
                    && tspan == name_span
                {
                    self.push(name_span.clone(), true);
                }
            }
            Decl::Import { path, quoted, .. } => {
                if let Symbol::ImportPath(target) = self.symbol
                    && target == path
                    && let Some(span) = locate_import_path(self.source, &d.span, path, *quoted)
                {
                    self.push(span, false);
                }
            }
        }
    }

    /// Class / Interface header scan: collect references to a target
    /// class/interface name appearing in `extends X` or
    /// `implements A, B, C`. We bound the search to the source slice
    /// from the start of the decl up to (but not including) the first
    /// member, where headers always live.
    pub(crate) fn collect_type_name_refs_in_header(
        &mut self,
        d: &Spanned<Decl>,
        extends: Option<&str>,
        implements: &[String],
    ) {
        let target_name = match self.symbol {
            Symbol::Class(n) | Symbol::Interface(n) | Symbol::Enum(n) => n,
            _ => return,
        };
        let head_end = match &d.value {
            Decl::Class { members, .. } => {
                members.first().map(|m| m.span.start).unwrap_or(d.span.end)
            }
            Decl::Interface { methods, .. } => {
                methods.first().map(|m| m.span.start).unwrap_or(d.span.end)
            }
            _ => d.span.end,
        };
        let head = d.span.start..head_end;
        let in_extends = extends == Some(target_name);
        let in_implements = implements.iter().any(|n| n == target_name);
        if !in_extends && !in_implements {
            return;
        }
        for span in locate_words_in(self.source, &head, target_name) {
            // Skip the decl-name occurrence we already handled.
            if let Some(name_span) = locate_word_in(self.source, &d.span, declared_name(&d.value))
                && name_span == span
            {
                continue;
            }
            self.push(span, false);
        }
    }

    pub(crate) fn visit_member(&mut self, m: &Spanned<ClassMember>) {
        let class = self.enclosing_class.clone().unwrap_or_default();
        match &m.value {
            ClassMember::Field {
                name, ty, default, ..
            } => {
                if let Symbol::Field {
                    class: tc,
                    name: tn,
                } = self.symbol
                    && tc == &class
                    && tn == name
                    && let Some(span) = locate_word_in(self.source, &m.span, name)
                {
                    self.push(span, true);
                }
                self.collect_type_name_refs_in(ty, &m.span);
                if let Some(def) = default {
                    self.visit_expr(def);
                }
            }
            ClassMember::Method(meth) => self.visit_method(meth, &class),
        }
    }

    pub(crate) fn visit_method(&mut self, meth: &Method, class: &str) {
        if let Symbol::Method {
            class: tc,
            name: tn,
        } = self.symbol
            && tc == class
            && tn == &meth.name
            && let Some(span) = locate_word_in(self.source, &meth.span, &meth.name)
        {
            self.push(span, true);
        }
        if let Some(rt) = &meth.return_ty {
            let after = meth
                .params
                .last()
                .map(|p| p.span.end)
                .unwrap_or(meth.span.start);
            let before = meth
                .body
                .first()
                .map(|s| s.span.start)
                .unwrap_or(meth.span.end);
            self.collect_type_name_refs_in(rt, &(after..before));
        }
        self.enter_function(&meth.params, |this| {
            for p in &meth.params {
                this.collect_type_name_refs_in(&p.ty, &p.span);
                if let Some(def) = &p.default {
                    this.visit_expr(def);
                }
            }
            for s in &meth.body {
                this.visit_stmt(s);
            }
        });
    }
}
