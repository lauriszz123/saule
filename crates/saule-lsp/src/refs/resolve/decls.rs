//! Walking declarations: functions, classes, their members and
//! methods, and the type names in a class header.

use crate::refs::Symbol;
use crate::refs::util::{contains, locate_import_path, locate_word_in};
use saule_ast::{ClassMember, Decl, EnumVariant, Method, Spanned};

use super::*;

impl<'a> ResolveCx<'a> {
    pub(crate) fn visit_decl(&mut self, d: &Spanned<Decl>) {
        if !contains(&d.span, self.offset) {
            return;
        }
        match &d.value {
            Decl::Function {
                name, params, body, ..
            } => {
                if let Some(span) = locate_word_in(self.source, &d.span, name) {
                    self.record(span, Symbol::Function(name.clone()));
                }
                self.enter_function(params, |this| {
                    for p in params {
                        if let Some(span) = locate_word_in(this.source, &p.span, &p.name) {
                            this.record(
                                span.clone(),
                                Symbol::Local {
                                    name: p.name.clone(),
                                    def_span: span,
                                },
                            );
                        }
                        if let Some(def) = &p.default {
                            this.visit_expr(def);
                        }
                    }
                    for s in body {
                        this.visit_stmt(s);
                    }
                });
            }
            Decl::Class { name, members, .. } => {
                if let Some(span) = locate_word_in(self.source, &d.span, name) {
                    self.record(span, Symbol::Class(name.clone()));
                }
                let prev = self.enclosing_class.replace(name.clone());
                for m in members {
                    self.visit_member(m);
                }
                self.enclosing_class = prev;
            }
            Decl::Interface { name, .. } => {
                if let Some(span) = locate_word_in(self.source, &d.span, name) {
                    self.record(span, Symbol::Interface(name.clone()));
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

    pub(crate) fn visit_member(&mut self, m: &Spanned<ClassMember>) {
        if !contains(&m.span, self.offset) {
            return;
        }
        let class = self.enclosing_class.clone().unwrap_or_default();
        match &m.value {
            ClassMember::Field { name, default, .. } => {
                if let Some(span) = locate_word_in(self.source, &m.span, name) {
                    self.record(
                        span,
                        Symbol::Field {
                            class: class.clone(),
                            name: name.clone(),
                        },
                    );
                }
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
        self.enter_function(&meth.params, |this| {
            for p in &meth.params {
                if let Some(span) = locate_word_in(this.source, &p.span, &p.name) {
                    this.record(
                        span.clone(),
                        Symbol::Local {
                            name: p.name.clone(),
                            def_span: span,
                        },
                    );
                }
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
