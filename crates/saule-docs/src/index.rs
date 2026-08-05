//! Walking a [`Module`] for documentable declarations.
//!
//! [`walk`] is the shared traversal: it yields one [`DocItem`] per thing
//! a user can write a `---` block above, carrying the qualified name,
//! the byte offset to scan up from, and the parameter list to check
//! `@param` tags against. [`collect`] turns that into a name-keyed
//! [`DocIndex`]; [`crate::validate`] uses the same walk to look for
//! `@param` tags that don't match any parameter.
//!
//! Only top-level declarations and their members are visited. A `local
//! fn` nested inside a block is not indexed — hover can still document
//! it by calling [`crate::extract`] directly with the node's span, which
//! is how the LSP reaches nodes the index doesn't name.

use std::collections::HashMap;

use saule_ast::{ClassMember, Decl, EnumVariant, Module, Param, Stmt};

use crate::{DocBlock, extract};

/// What kind of declaration a [`DocItem`] describes. Consumers use this
/// to pick a completion-item kind or a hover heading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Function,
    Class,
    Interface,
    Enum,
    Method,
    Field,
    Variant,
    /// A module-level `export name: T = value`.
    Variable,
}

/// A declaration that may carry a doc comment.
#[derive(Debug, Clone)]
pub struct DocItem<'a> {
    /// `Entity`, `Entity.init`, `Entity.age`, `Direction.North`.
    pub qname: String,
    /// Byte offset inside the declaration; [`crate::extract`] scans
    /// upward from this line.
    pub anchor: usize,
    /// Declared parameters, empty for anything that isn't callable.
    pub params: &'a [Param],
    pub kind: ItemKind,
}

/// Every documentable declaration in `module`, in source order.
pub fn walk(module: &Module) -> Vec<DocItem<'_>> {
    const NONE: &[Param] = &[];
    let mut out = Vec::new();

    for stmt in &module.stmts {
        let Stmt::Decl(d) = &stmt.value else { continue };
        let anchor = d.span.start;

        match &d.value {
            Decl::Function { name, params, .. } => out.push(DocItem {
                qname: name.clone(),
                anchor,
                params,
                kind: ItemKind::Function,
            }),

            Decl::Class { name, members, .. } => {
                out.push(DocItem {
                    qname: name.clone(),
                    anchor,
                    params: NONE,
                    kind: ItemKind::Class,
                });
                for m in members {
                    // Anchor on the `Spanned<ClassMember>` rather than
                    // `Method::span`: the outer span starts at the
                    // `local` / `static` modifier, the inner one at `fn`.
                    // Both land on the same line, but the outer is the
                    // honest start of the member.
                    let at = m.span.start;
                    match &m.value {
                        ClassMember::Field { name: f, .. } => out.push(DocItem {
                            qname: format!("{name}.{f}"),
                            anchor: at,
                            params: NONE,
                            kind: ItemKind::Field,
                        }),
                        ClassMember::Method(me) => out.push(DocItem {
                            qname: format!("{name}.{}", me.name),
                            anchor: at,
                            params: &me.params,
                            kind: ItemKind::Method,
                        }),
                    }
                }
            }

            Decl::Variable { name, .. } => out.push(DocItem {
                qname: name.clone(),
                anchor,
                params: NONE,
                kind: ItemKind::Variable,
            }),

            Decl::Interface { name, methods, .. } => {
                out.push(DocItem {
                    qname: name.clone(),
                    anchor,
                    params: NONE,
                    kind: ItemKind::Interface,
                });
                for m in methods {
                    out.push(DocItem {
                        qname: format!("{name}.{}", m.name),
                        anchor: m.span.start,
                        params: &m.params,
                        kind: ItemKind::Method,
                    });
                }
            }

            Decl::Enum {
                name,
                variants,
                methods,
                ..
            } => {
                out.push(DocItem {
                    qname: name.clone(),
                    anchor,
                    params: NONE,
                    kind: ItemKind::Enum,
                });
                for v in variants {
                    let (vname, fields) = match &v.value {
                        EnumVariant::Bare(n) => (n, NONE),
                        EnumVariant::Valued(n, _) => (n, NONE),
                        // A tuple variant is constructed like a call, so
                        // its fields are legitimate `@param` targets.
                        EnumVariant::Tuple { name: n, fields } => (n, fields.as_slice()),
                    };
                    out.push(DocItem {
                        qname: format!("{name}.{vname}"),
                        anchor: v.span.start,
                        params: fields,
                        kind: ItemKind::Variant,
                    });
                }
                for m in methods {
                    out.push(DocItem {
                        qname: format!("{name}.{}", m.name),
                        anchor: m.span.start,
                        params: &m.params,
                        kind: ItemKind::Method,
                    });
                }
            }

            Decl::Import { .. } => {}
        }
    }

    out
}

/// Doc comments for one module, keyed by qualified name.
#[derive(Debug, Clone, Default)]
pub struct DocIndex {
    entries: HashMap<String, DocBlock>,
}

impl DocIndex {
    /// The block documenting `qname`, if it has one.
    pub fn get(&self, qname: &str) -> Option<&DocBlock> {
        self.entries.get(qname)
    }

    /// Summary line for `qname` — the common case for a completion
    /// item's documentation field.
    pub fn summary(&self, qname: &str) -> Option<&str> {
        self.entries
            .get(qname)
            .map(|d| d.summary.as_str())
            .filter(|s| !s.is_empty())
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Fold `other` in, keeping entries already present.
    ///
    /// Used to layer imported modules' docs underneath the current
    /// file's: a name declared locally shadows the imported one in
    /// scope, so its doc should win here too.
    pub fn merge(&mut self, other: DocIndex) {
        for (k, v) in other.entries {
            self.entries.entry(k).or_insert(v);
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &DocBlock)> {
        self.entries.iter()
    }
}

/// Build the [`DocIndex`] for `module`. Declarations without a doc
/// comment — and blocks that turn out to be empty — are skipped, so a
/// lookup miss and "documented with nothing" are the same thing.
pub fn collect(module: &Module, source: &str) -> DocIndex {
    let mut entries = HashMap::new();
    for item in walk(module) {
        if let Some(block) = extract(source, item.anchor)
            && !block.is_empty()
        {
            entries.insert(item.qname, block);
        }
    }
    DocIndex { entries }
}
