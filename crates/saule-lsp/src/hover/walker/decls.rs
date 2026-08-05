//! Walking declarations — functions, classes, members and methods.

use crate::hover::render::{
    render_class_full, render_class_head, render_enum_head, render_enum_variant_decl, render_field,
    render_function_sig, render_interface_head, render_interface_method, render_method_head,
    render_param, with_doc, with_param_doc,
};
use crate::hover::util::{contains, locate_word_in};
use saule_ast::{ClassMember, Decl, Method, Spanned};
use saule_semantic::with_classes;

use super::*;

impl<'a> Cx<'a> {
    pub(crate) fn visit_decl(&mut self, d: &Spanned<Decl>) {
        // Skip the whole subtree when the declaration's span doesn't
        // even contain the cursor — saves an analysis on every
        // unrelated top-level item in a long file.
        if !contains(&d.span, self.offset) {
            return;
        }
        match &d.value {
            Decl::Function {
                name,
                type_params,
                params,
                return_ty,
                body,
                ..
            } => {
                let doc = self.doc_at(d.span.start);
                let head_end = params
                    .first()
                    .map(|p| p.span.start)
                    .or_else(|| body.first().map(|s| s.span.start))
                    .unwrap_or(d.span.end);
                let name_span = self.decl_name_span(&d.span, name, head_end);
                self.record(
                    name_span,
                    with_doc(
                        render_function_sig(name, type_params, params, return_ty.as_ref()),
                        doc.as_ref(),
                    ),
                );
                for p in params {
                    self.record(
                        p.span.clone(),
                        with_param_doc(render_param(p), doc.as_ref(), &p.name),
                    );
                    // Type ascription on the param: `x: Storage` —
                    // make `Storage` itself hoverable.
                    self.record_type_idents_in(&p.ty, &p.span);
                    if let Some(def) = &p.default {
                        self.visit_expr(def);
                    }
                }
                if let Some(rt) = return_ty {
                    // Locate the return type slice between the closing
                    // `)` of the param list and the start of the body.
                    let after = params.last().map(|p| p.span.end).unwrap_or(d.span.start);
                    let before = body.first().map(|s| s.span.start).unwrap_or(d.span.end);
                    self.record_type_idents_in(rt, &(after..before));
                }
                let params = params.clone();
                let return_ty = return_ty.clone();
                self.enter_function_with_return(&params, return_ty.as_ref(), |this| {
                    this.visit_block(body)
                });
            }
            Decl::Class {
                name,
                extends,
                implements,
                members,
                ..
            } => {
                // Prefer the registry view (uniform with how `Ident`
                // hover renders the same class), falling back to the
                // raw AST head when the registry is empty — e.g. when
                // hover is invoked on a file whose semantic pass
                // hasn't run yet.
                let md = with_classes(|r| r.get(name).cloned())
                    .map(|info| render_class_full(name, &info))
                    .unwrap_or_else(|| render_class_head(name, extends.as_deref(), implements));
                let doc = self.doc_at(d.span.start);
                let header_end = members.first().map(|m| m.span.start).unwrap_or(d.span.end);
                let name_span = self.decl_name_span(&d.span, name, header_end);
                self.record(name_span, with_doc(md, doc.as_ref()));
                // Resolve `extends X` / `implements Y, Z` parent and
                // interface names within the class header (between
                // the class name and the first member). We use the
                // first occurrence of each name within the decl span
                // as the hover target.
                let header_span = d.span.start..header_end;
                if let Some(parent) = extends {
                    self.record_named_idents_in(std::slice::from_ref(parent), &header_span);
                }
                self.record_named_idents_in(implements, &header_span);
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
                let doc = self.doc_at(d.span.start);
                let head_end = methods.first().map(|m| m.span.start).unwrap_or(d.span.end);
                let name_span = self.decl_name_span(&d.span, name, head_end);
                self.record(
                    name_span,
                    with_doc(render_interface_head(name, extends, methods), doc.as_ref()),
                );
                // Same idea as class `extends` — locate parent
                // interface names within the interface decl span.
                self.record_named_idents_in(extends, &d.span);
                // Each bodiless method is a hover target of its own so
                // a `---` block written above it has somewhere to go.
                for m in methods {
                    if !contains(&m.span, self.offset) {
                        continue;
                    }
                    let mdoc = self.doc_at(m.span.start);
                    self.record(
                        m.span.clone(),
                        with_doc(render_interface_method(name, m), mdoc.as_ref()),
                    );
                    for p in &m.params {
                        self.record(
                            p.span.clone(),
                            with_param_doc(render_param(p), mdoc.as_ref(), &p.name),
                        );
                        self.record_type_idents_in(&p.ty, &p.span);
                    }
                    if let Some(rt) = &m.return_ty {
                        let after = m.params.last().map(|p| p.span.end).unwrap_or(m.span.start);
                        self.record_type_idents_in(rt, &(after..m.span.end));
                    }
                }
            }
            Decl::Enum {
                name,
                variants,
                methods,
                ..
            } => {
                let doc = self.doc_at(d.span.start);
                let head_end = variants.first().map(|v| v.span.start).unwrap_or(d.span.end);
                let name_span = self.decl_name_span(&d.span, name, head_end);
                self.record(
                    name_span,
                    with_doc(render_enum_head(name, variants), doc.as_ref()),
                );
                for v in variants {
                    if !contains(&v.span, self.offset) {
                        continue;
                    }
                    let vdoc = self.doc_at(v.span.start);
                    self.record(
                        v.span.clone(),
                        with_doc(render_enum_variant_decl(name, &v.value), vdoc.as_ref()),
                    );
                    if let saule_ast::EnumVariant::Tuple { fields, .. } = &v.value {
                        for p in fields {
                            self.record(
                                p.span.clone(),
                                with_param_doc(render_param(p), vdoc.as_ref(), &p.name),
                            );
                            self.record_type_idents_in(&p.ty, &p.span);
                        }
                    }
                }
                let prev = self.enclosing_class.replace(name.clone());
                for m in methods {
                    self.visit_method(m, name);
                }
                self.enclosing_class = prev;
            }
            Decl::Variable {
                name,
                name_span,
                ty,
                ty_span,
                value,
                ..
            } => {
                if let Some(v) = value {
                    self.visit_expr(v);
                }
                let doc = self.doc_at(d.span.start);
                let resolved = ty
                    .clone()
                    .or_else(|| value.as_ref().and_then(|v| self.infer_init_type(&v.value)))
                    .unwrap_or_else(|| saule_ast::Type::Named("any".into()));
                self.record(
                    name_span.clone(),
                    with_doc(
                        format!(
                            "```saule\n(export) {name}: {ty}\n```",
                            ty = crate::hover::render::render_type(&resolved)
                        ),
                        doc.as_ref(),
                    ),
                );
                // Cursor on the annotation itself (`export s: Storage = …`
                // -> hover on `Storage`).
                if let Some(span) = ty_span
                    && let Some(t) = ty.as_ref()
                {
                    self.record_type_idents_in(t, span);
                }
            }
            Decl::Import { names, .. } => {
                // Best match wins: walk the precomputed blurbs and
                // record any whose span contains the cursor. Spans
                // come from the `Spanned<Decl>` itself, so they cover
                // the full statement.
                for (span, md) in &self.imports.import_blurbs {
                    if contains(span, self.offset) {
                        self.record(span.clone(), md.clone());
                    }
                }
                // Per-name resolution: if the cursor is on one of the
                // listed import names, prefer its specific class /
                // interface / enum / function blurb over the generic
                // statement-wide one. Aliased imports (`X as Y`)
                // resolve through the alias name the user typed.
                if let saule_ast::ImportNames::List(items) = names {
                    for (orig, alias) in items {
                        let local = alias.as_deref().unwrap_or(orig);
                        // Local alias span — the name the importer
                        // sees and would hover.
                        if let Some(span) = locate_word_in(self.source, &d.span, local)
                            && contains(&span, self.offset)
                            && let Some(md) = self.resolve_ident(local)
                        {
                            self.record(span, md);
                        }
                        // Original (upstream) name when an alias was
                        // used: `import X as Y` — both should hover.
                        if alias.is_some()
                            && let Some(span) = locate_word_in(self.source, &d.span, orig)
                            && contains(&span, self.offset)
                            && let Some(md) = self.resolve_ident(orig)
                        {
                            self.record(span, md);
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn visit_member(&mut self, m: &Spanned<ClassMember>) {
        if !contains(&m.span, self.offset) {
            return;
        }
        match &m.value {
            ClassMember::Field {
                is_static,
                is_private,
                name,
                ty,
                default,
            } => {
                let owner = self.enclosing_class.as_deref().unwrap_or("");
                let doc = self.doc_at(m.span.start);
                self.record(
                    m.span.clone(),
                    with_doc(
                        render_field(owner, *is_static, *is_private, name, ty),
                        doc.as_ref(),
                    ),
                );
                // Make `due: integer?` -> hover on `integer` resolve
                // through the same path bare ident hovers use.
                self.record_type_idents_in(ty, &m.span);
                if let Some(def) = default {
                    self.visit_expr(def);
                }
            }
            ClassMember::Method(meth) => {
                let owner = self.enclosing_class.clone().unwrap_or_default();
                self.visit_method(meth, &owner);
            }
        }
    }

    pub(crate) fn visit_method(&mut self, m: &Method, owner: &str) {
        if !contains(&m.span, self.offset) {
            return;
        }
        let doc = self.doc_at(m.span.start);
        let head_end = m
            .params
            .first()
            .map(|p| p.span.start)
            .or_else(|| m.body.first().map(|s| s.span.start))
            .unwrap_or(m.span.end);
        let name_span = self.decl_name_span(&m.span, &m.name, head_end);
        self.record(
            name_span,
            with_doc(render_method_head(owner, m), doc.as_ref()),
        );
        for p in &m.params {
            self.record(
                p.span.clone(),
                with_param_doc(render_param(p), doc.as_ref(), &p.name),
            );
            self.record_type_idents_in(&p.ty, &p.span);
            if let Some(def) = &p.default {
                self.visit_expr(def);
            }
        }
        if let Some(rt) = &m.return_ty {
            let after = m.params.last().map(|p| p.span.end).unwrap_or(m.span.start);
            let before = m.body.first().map(|s| s.span.start).unwrap_or(m.span.end);
            self.record_type_idents_in(rt, &(after..before));
        }
        let params = m.params.clone();
        let return_ty = m.return_ty.clone();
        self.enter_function_with_return(&params, return_ty.as_ref(), |this| {
            this.visit_block(&m.body)
        });
    }
}
