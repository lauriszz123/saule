//! `saule-docs` — doc comments for Saule declarations.
//!
//! A **doc comment** is a run of `---` line comments sitting immediately
//! above a declaration:
//!
//! ```saule
//! --- A base class for all entities.
//! class Entity
//!   --- How many ticks this entity has survived.
//!   local age: integer = 0
//!
//!   --- Build an entity.
//!   --- @param name The display name, shown in the HUD.
//!   --- @return The freshly-spawned entity.
//!   fn init(name: string)
//!   end
//! end
//! ```
//!
//! ## Why `---` costs nothing
//!
//! `---` is already a valid `--` line comment, so the lexer needs no
//! change: [`saule_lexer`] hands back a `LineComment` whose text is
//! everything after the first two dashes. Old sources stay valid by
//! construction and the formatter keeps round-tripping doc comments as
//! ordinary trivia.
//!
//! The marker is *exactly* three dashes not followed by a fourth — the
//! same escape hatch Rust gives you with `////`. So a decorative rule
//! (`--------------------`) or a banner (`---- Section ----`) stays a
//! plain comment and never attaches itself to whatever follows it.
//!
//! ## Extraction, not parsing
//!
//! Docs are recovered from the source text by scanning *upward* from a
//! declaration's span rather than by threading comment trivia through
//! the parser. Every documentable node already carries a span
//! (`Spanned<Decl>`, `Spanned<ClassMember>`, [`Method::span`],
//! [`MethodSig::span`], `Spanned<EnumVariant>`), so this needs no change
//! to the AST, the parser, or the formatter — which in turn means
//! `Decl`'s `PartialEq` and the existing parser tests are untouched.
//!
//! Two entry points:
//!
//! * [`extract`] — the primitive. Given the source and a byte offset
//!   anywhere in a declaration, return its [`DocBlock`] (or `None`).
//! * [`collect`] — walk a whole [`Module`] and build a [`DocIndex`]
//!   keyed by qualified name (`Entity`, `Entity.init`, `Entity.age`,
//!   `Direction.North`).
//!
//! [`validate`] pairs each block against the declaration it documents
//! and reports `@param` tags naming a parameter that does not exist.
//!
//! [`Method::span`]: saule_ast::Method::span
//! [`MethodSig::span`]: saule_ast::MethodSig::span
//! [`Module`]: saule_ast::Module

mod extract;
mod index;
mod render;
#[cfg(test)]
mod tests;
mod validate;

pub use extract::extract;
pub use index::{DocIndex, ItemKind, collect, walk};
pub use validate::{DocWarning, validate};

use std::ops::Range;

/// The parsed contents of one `---` run.
///
/// `summary` is everything the tag parser did not claim, joined with
/// newlines and kept as Markdown — including any `@tag` we don't
/// understand, so writing `@deprecated` today renders verbatim instead
/// of being silently eaten.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocBlock {
    /// Free-form Markdown description, tags removed.
    pub summary: String,
    /// `@param <name> <desc>` entries, in source order.
    pub params: Vec<ParamDoc>,
    /// `@return <desc>` (also spelled `@returns`). Last one wins.
    pub returns: Option<String>,
    /// Byte range of the whole `---` run in the source.
    pub span: Range<usize>,
}

/// One `@param` tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamDoc {
    /// The parameter name as written in the tag.
    pub name: String,
    /// Description text, with continuation lines folded in.
    pub desc: String,
    /// Byte range of just the name token, so a diagnostic can underline
    /// the offending word rather than the whole line.
    pub name_span: Range<usize>,
}

impl DocBlock {
    /// `true` when the block carries no usable content — an empty `---`
    /// run, or one holding only blank lines.
    pub fn is_empty(&self) -> bool {
        self.summary.trim().is_empty() && self.params.is_empty() && self.returns.is_none()
    }

    /// Look up the description for `name`.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.desc.as_str())
    }
}
