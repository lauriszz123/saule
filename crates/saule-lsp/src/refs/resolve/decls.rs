//! Walking declarations: functions, classes, their members and
//! methods, and the type names in a class header.

use crate::refs::Symbol;
use crate::refs::util::{contains, locate_import_path, locate_word_in, type_name_symbol};
use saule_ast::{ClassMember, Decl, EnumVariant, Method, MethodSig, Spanned};
use std::ops::Range;

use super::*;

impl<'a> ResolveCx<'a> {
    pub(crate) fn visit_decl(&mut self, d: &Spanned<Decl>) {
        if !contains(&d.span, self.offset) {
            return;
        }
        match &d.value {
            Decl::Function {
                name,
                params,
                return_ty,
                body,
                ..
            } => {
                if let Some(span) = locate_word_in(self.source, &d.span, name) {
                    self.record(span, Symbol::Function(name.clone()));
                }
                if let Some(rt) = return_ty {
                    let after = params.last().map(|p| p.span.end).unwrap_or(d.span.start);
                    let before = body.first().map(|s| s.span.start).unwrap_or(d.span.end);
                    self.record_type_names_in(rt, &(after..before));
                }
                self.enter_function(params, |this| {
                    for p in params {
                        this.visit_param(p);
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
                if let Some(span) = locate_word_in(self.source, &d.span, name) {
                    self.record(span, Symbol::Class(name.clone()));
                }
                let header_end = members.first().map(|m| m.span.start).unwrap_or(d.span.end);
                self.record_header_names(
                    &(d.span.start..header_end),
                    extends.iter().chain(implements),
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
                if let Some(span) = locate_word_in(self.source, &d.span, name) {
                    self.record(span, Symbol::Interface(name.clone()));
                }
                let header_end = methods.first().map(|m| m.span.start).unwrap_or(d.span.end);
                self.record_header_names(&(d.span.start..header_end), extends);
                for m in methods {
                    self.visit_method_sig(m);
                }
            }
            Decl::Enum {
                name,
                variants,
                methods,
                ..
            } => {
                if let Some(span) = locate_word_in(self.source, &d.span, name) {
                    self.record(span, Symbol::Enum(name.clone()));
                }
                for v in variants {
                    let (vname, fields) = match &v.value {
                        EnumVariant::Bare(n) | EnumVariant::Valued(n, _) => (n.as_str(), None),
                        EnumVariant::Tuple { name, fields } => (name.as_str(), Some(fields)),
                    };
                    if let Some(span) = locate_word_in(self.source, &v.span, vname) {
                        self.record(
                            span,
                            Symbol::EnumVariant {
                                enum_name: name.clone(),
                                variant: vname.to_string(),
                            },
                        );
                    }
                    if let Some(fs) = fields {
                        for p in fs {
                            self.record_type_names_in(&p.ty, &p.span);
                            if let Some(def) = &p.default {
                                self.visit_expr(def);
                            }
                        }
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
            Decl::Import { path, quoted, .. } => {
                // Find the path inside the import statement — quoted between
                // the matching quotes, bare at the end of the declaration.
                if let Some(span) = locate_import_path(self.source, &d.span, path, *quoted) {
                    self.record(span, Symbol::ImportPath(path.clone()));
                }
            }
        }
    }

    /// Record the type names listed in a `extends` / `implements`
    /// header as references to whatever they denote. The registries
    /// decide the kind, so `class C extends Base implements Printable`
    /// yields a `Class` for one and an `Interface` for the other.
    pub(crate) fn record_header_names<'n>(
        &mut self,
        header: &Range<usize>,
        names: impl IntoIterator<Item = &'n String>,
    ) {
        if !contains(header, self.offset) {
            return;
        }
        for name in names {
            let Some(span) = locate_word_in(self.source, header, name) else {
                continue;
            };
            if !contains(&span, self.offset) {
                continue;
            }
            if let Some(sym) = type_name_symbol(name) {
                self.record(span, sym);
            }
        }
    }

    pub(crate) fn visit_member(&mut self, m: &Spanned<ClassMember>) {
        if !contains(&m.span, self.offset) {
            return;
        }
        let class = self.enclosing_class.clone().unwrap_or_default();
        match &m.value {
            ClassMember::Field {
                name, ty, default, ..
            } => {
                if let Some(span) = locate_word_in(self.source, &m.span, name) {
                    self.record(
                        span,
                        Symbol::Field {
                            class: class.clone(),
                            name: name.clone(),
                        },
                    );
                }
                self.record_type_names_in(ty, &m.span);
                if let Some(def) = default {
                    self.visit_expr(def);
                }
            }
            ClassMember::Method(meth) => self.visit_method(meth, &class),
        }
    }

    pub(crate) fn visit_method(&mut self, meth: &Method, class: &str) {
        if !contains(&meth.span, self.offset) {
            return;
        }
        if let Some(span) = locate_word_in(self.source, &meth.span, &meth.name) {
            self.record(
                span,
                Symbol::Method {
                    class: class.to_string(),
                    name: meth.name.clone(),
                },
            );
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
            self.record_type_names_in(rt, &(after..before));
        }
        self.enter_function(&meth.params, |this| {
            for p in &meth.params {
                this.visit_param(p);
            }
            for s in &meth.body {
                this.visit_stmt(s);
            }
        });
    }

    /// An interface method has no body — only its parameter and return
    /// types are navigable.
    pub(crate) fn visit_method_sig(&mut self, sig: &MethodSig) {
        if !contains(&sig.span, self.offset) {
            return;
        }
        for p in &sig.params {
            self.record_type_names_in(&p.ty, &p.span);
        }
        if let Some(rt) = &sig.return_ty {
            let after = sig
                .params
                .last()
                .map(|p| p.span.end)
                .unwrap_or(sig.span.start);
            self.record_type_names_in(rt, &(after..sig.span.end));
        }
    }
}
