//! Document symbol provider — produces the editor's "outline" /
//! "breadcrumbs" view by walking the AST and emitting one
//! [`DocumentSymbol`] per top-level declaration, with class methods /
//! fields and enum variants nested as children.
//!
//! Symbol *ranges* cover the entire declaration span (so clicking on
//! the outline takes you to the start of the decl); *selection ranges*
//! cover just the name token, recovered by scanning the source for the
//! first occurrence of the name inside the decl span.

use saule_ast::{ClassMember, Decl, EnumVariant, Method, Module, Stmt};
use tower_lsp::lsp_types::{DocumentSymbol, SymbolKind, Url};

use crate::line_index::LineIndex;
use crate::server::sighelp::render_type;

use super::Backend;

impl Backend {
    /// Build the outline for `uri` from the cached source. Returns
    /// `None` when the document is closed or fails to parse.
    pub(super) async fn document_symbols(&self, uri: &Url) -> Option<Vec<DocumentSymbol>> {
        let entry = self.docs.get(uri.as_str())?;
        let source = entry.source.clone();
        drop(entry);

        let module = self.syntax(uri, &source);
        let line_index = LineIndex::new(&source);
        Some(build(&module, &source, &line_index))
    }
}

fn build(module: &Module, source: &str, idx: &LineIndex) -> Vec<DocumentSymbol> {
    let mut out = Vec::new();
    for stmt in &module.stmts {
        let Stmt::Decl(d) = &stmt.value else {
            continue;
        };
        match &d.value {
            Decl::Function {
                name,
                params,
                return_ty,
                ..
            } => {
                out.push(symbol(
                    name,
                    SymbolKind::FUNCTION,
                    Some(detail_function(params.len(), return_ty.is_some())),
                    &d.span,
                    name,
                    source,
                    idx,
                    None,
                ));
            }
            Decl::Class {
                name,
                extends,
                implements,
                members,
                ..
            } => {
                let mut children = Vec::new();
                for m in members {
                    match &m.value {
                        ClassMember::Field { name, .. } => {
                            children.push(symbol(
                                name,
                                SymbolKind::FIELD,
                                None,
                                &m.span,
                                name,
                                source,
                                idx,
                                None,
                            ));
                        }
                        ClassMember::Method(meth) => {
                            children.push(method_symbol(meth, source, idx));
                        }
                    }
                }
                out.push(symbol(
                    name,
                    SymbolKind::CLASS,
                    Some(detail_class(extends.as_deref(), implements)),
                    &d.span,
                    name,
                    source,
                    idx,
                    Some(children),
                ));
            }
            Decl::Interface {
                name,
                methods,
                extends,
                ..
            } => {
                let mut children = Vec::new();
                for m in methods {
                    children.push(symbol(
                        &m.name,
                        SymbolKind::METHOD,
                        Some(detail_function(m.params.len(), m.return_ty.is_some())),
                        &m.span,
                        &m.name,
                        source,
                        idx,
                        None,
                    ));
                }
                out.push(symbol(
                    name,
                    SymbolKind::INTERFACE,
                    Some(detail_interface(extends)),
                    &d.span,
                    name,
                    source,
                    idx,
                    Some(children),
                ));
            }
            Decl::Enum {
                name,
                variants,
                methods,
                ..
            } => {
                let mut children = Vec::new();
                for v in variants {
                    let vname = match &v.value {
                        EnumVariant::Bare(n) => n,
                        EnumVariant::Valued(n, _) => n,
                        EnumVariant::Tuple { name, .. } => name,
                    };
                    children.push(symbol(
                        vname,
                        SymbolKind::ENUM_MEMBER,
                        None,
                        &v.span,
                        vname,
                        source,
                        idx,
                        None,
                    ));
                }
                for m in methods {
                    children.push(method_symbol(m, source, idx));
                }
                out.push(symbol(
                    name,
                    SymbolKind::ENUM,
                    None,
                    &d.span,
                    name,
                    source,
                    idx,
                    Some(children),
                ));
            }
            Decl::Variable { name, ty, .. } => {
                out.push(symbol(
                    name,
                    SymbolKind::VARIABLE,
                    ty.as_ref().map(render_type),
                    &d.span,
                    name,
                    source,
                    idx,
                    None,
                ));
            }
            Decl::Import { .. } => {
                // Imports are not part of the outline — they aren't
                // symbols the user navigates to from the breadcrumb
                // bar. (Goto-definition still works on the import
                // path text via `nav.rs`.)
            }
        }
    }
    out
}

fn method_symbol(m: &Method, source: &str, idx: &LineIndex) -> DocumentSymbol {
    let kind = if m.is_static {
        SymbolKind::FUNCTION
    } else {
        SymbolKind::METHOD
    };
    symbol(
        &m.name,
        kind,
        Some(detail_function(m.params.len(), m.return_ty.is_some())),
        &m.span,
        &m.name,
        source,
        idx,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn symbol(
    name: &str,
    kind: SymbolKind,
    detail: Option<String>,
    full_span: &std::ops::Range<usize>,
    selection_name: &str,
    source: &str,
    idx: &LineIndex,
    children: Option<Vec<DocumentSymbol>>,
) -> DocumentSymbol {
    let sel_span = locate_word(source, full_span, selection_name).unwrap_or(full_span.clone());
    #[allow(deprecated)]
    DocumentSymbol {
        name: name.to_string(),
        detail,
        kind,
        tags: None,
        deprecated: None,
        range: idx.range(source, full_span.start, full_span.end),
        selection_range: idx.range(source, sel_span.start, sel_span.end),
        children,
    }
}

/// First whole-word occurrence of `name` inside `range`. Lifted from
/// [`crate::hover::util`] but redeclared locally to avoid widening
/// that module's `pub(super)` boundary.
fn locate_word(
    source: &str,
    range: &std::ops::Range<usize>,
    name: &str,
) -> Option<std::ops::Range<usize>> {
    let end = range.end.min(source.len());
    let start = range.start.min(end);
    let slice = source.get(start..end)?;
    let bytes = slice.as_bytes();
    let pat = name.as_bytes();
    if pat.is_empty() || pat.len() > bytes.len() {
        return None;
    }
    let mut i = 0;
    while i + pat.len() <= bytes.len() {
        if &bytes[i..i + pat.len()] == pat {
            let before_ok = i == 0 || !is_ident_byte(bytes[i - 1]);
            let after_ok = i + pat.len() == bytes.len() || !is_ident_byte(bytes[i + pat.len()]);
            if before_ok && after_ok {
                return Some((start + i)..(start + i + pat.len()));
            }
        }
        i += 1;
    }
    None
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn detail_function(arity: usize, has_return: bool) -> String {
    let suffix = if has_return { " -> _" } else { "" };
    format!("({arity} args){suffix}")
}

fn detail_class(extends: Option<&str>, implements: &[String]) -> String {
    let mut parts = Vec::new();
    if let Some(p) = extends {
        parts.push(format!("extends {p}"));
    }
    if !implements.is_empty() {
        parts.push(format!("implements {}", implements.join(", ")));
    }
    parts.join(" ")
}

fn detail_interface(extends: &[String]) -> String {
    if extends.is_empty() {
        String::new()
    } else {
        format!("extends {}", extends.join(", "))
    }
}

#[cfg(test)]
mod tests {
    //! Outline tests — drive `build` directly with a parsed module so
    //! we don't need a live `Backend`. Asserts on (name, kind, child
    //! names) tuples, which is enough to catch regressions in the
    //! nesting / kind-mapping logic without coupling to span numbers.

    use super::*;

    /// The outline the editor would show for a *broken* buffer, given what
    /// the file looked like the last time it parsed cleanly. `None` for
    /// `prior_src` is a file the editor has never seen in a valid state.
    fn outline_broken(prior_src: Option<&str>, src: &str) -> Vec<String> {
        let prior = prior_src.map(|s| {
            let tokens = saule_lexer::Lexer::new(s).tokenize().expect("lex");
            let module = saule_parser::parse(tokens).expect("prior must parse");
            saule_parser::PriorShape::of(&module)
        });
        let module = crate::syntax::tolerant_with_prior(src, prior.as_ref());
        build(&module, src, &LineIndex::new(src))
            .iter()
            .map(|s| s.name.clone())
            .collect()
    }

    fn outline(src: &str) -> Vec<DocumentSymbol> {
        let tokens = saule_lexer::Lexer::new(src).tokenize().expect("lex");
        let module = saule_parser::parse(tokens).expect("parse");
        let idx = LineIndex::new(src);
        build(&module, src, &idx)
    }

    #[test]
    fn lists_top_level_function() {
        let src = "fn add(a: integer, b: integer) -> integer\n  return a + b\nend\n";
        let syms = outline(src);
        assert_eq!(syms.len(), 1);
        assert_eq!(syms[0].name, "add");
        assert_eq!(syms[0].kind, SymbolKind::FUNCTION);
        let detail = syms[0].detail.as_deref().unwrap_or("");
        assert!(detail.contains("2 args"), "detail={detail}");
        assert!(detail.contains("->"), "detail={detail}");
    }

    #[test]
    fn class_with_field_and_method_children() {
        let src = "class Point\n  x: integer = 0\n  fn dist() -> integer\n    return self.x\n  end\nend\n";
        let syms = outline(src);
        assert_eq!(syms.len(), 1);
        let cls = &syms[0];
        assert_eq!(cls.kind, SymbolKind::CLASS);
        assert_eq!(cls.name, "Point");
        let kids = cls.children.as_ref().expect("children");
        let names: Vec<_> = kids.iter().map(|c| (c.name.as_str(), c.kind)).collect();
        assert!(names.contains(&("x", SymbolKind::FIELD)), "{names:?}");
        assert!(names.contains(&("dist", SymbolKind::METHOD)), "{names:?}");
    }

    #[test]
    fn static_method_is_function_kind() {
        let src = "class Foo\n  static fn make() -> integer\n    return 1\n  end\nend\n";
        let syms = outline(src);
        let kids = syms[0].children.as_ref().unwrap();
        assert_eq!(kids[0].name, "make");
        assert_eq!(kids[0].kind, SymbolKind::FUNCTION);
    }

    #[test]
    fn interface_lists_method_signatures() {
        let src = "interface Shape\n  fn area() -> integer\n  fn name() -> string\nend\n";
        let syms = outline(src);
        assert_eq!(syms[0].kind, SymbolKind::INTERFACE);
        let kids = syms[0].children.as_ref().unwrap();
        assert_eq!(kids.len(), 2);
        assert!(kids.iter().all(|k| k.kind == SymbolKind::METHOD));
    }

    #[test]
    fn enum_lists_variant_children() {
        let src = "enum Color\n  Red\n  Green\n  Blue\nend\n";
        let syms = outline(src);
        assert_eq!(syms[0].kind, SymbolKind::ENUM);
        let kids = syms[0].children.as_ref().unwrap();
        let names: Vec<_> = kids.iter().map(|c| c.name.clone()).collect();
        assert_eq!(names, vec!["Red", "Green", "Blue"]);
        assert!(kids.iter().all(|k| k.kind == SymbolKind::ENUM_MEMBER));
    }

    #[test]
    fn imports_are_skipped() {
        let src = "import Foo from \"bar\"\nfn main()\nend\n";
        let syms = outline(src);
        assert_eq!(syms.len(), 1);
        assert_eq!(syms[0].name, "main");
    }

    // ─── The forgotten `end`, end to end ─────────────────────────────────────
    //
    // The outline is the clearest place to see what recovery buys, because it
    // is a direct rendering of tree shape: a declaration parsed one scope too
    // deep isn't shown nested, it vanishes — `build` doesn't descend into
    // function bodies looking for functions.

    const GOOD: &str = "fn before()\nlocal a = 1\nend\n\nfn after()\nlocal b = 2\nend\n";
    const BROKEN: &str = "fn before()\nlocal a = 1\n\nfn after()\nlocal b = 2\nend\n";

    /// Indentation is the only evidence inside the file, and an unindented
    /// file has none — so without history `after` drops out of the outline.
    #[test]
    fn an_unindented_file_alone_loses_the_declaration() {
        assert_eq!(outline_broken(None, BROKEN), ["before"]);
    }

    /// With the shape from the last clean parse of the same document, it
    /// stays. This is the whole point of remembering.
    #[test]
    fn history_keeps_it() {
        assert_eq!(outline_broken(Some(GOOD), BROKEN), ["before", "after"]);
    }

    /// An indented file never needed the history.
    #[test]
    fn indentation_alone_still_works() {
        let broken = "fn before()\n    local a = 1\n\nfn after()\n    local b = 2\nend\n";
        assert_eq!(outline_broken(None, broken), ["before", "after"]);
    }
}
