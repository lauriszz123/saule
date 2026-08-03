//! Saule source pretty-printer.
//!
//! Walks a parsed [`saule_ast::Module`] and renders it back to canonical
//! Saule source: 2-space indent, one statement per line, blank line between
//! top-level declarations.
//!
//! ## Comment preservation
//!
//! [`format_module`] discards comments — the AST never sees them. To round
//! trip comments, use [`format_module_with_comments`] together with the
//! lexer's `tokenize_with_trivia` entry point: extract every
//! [`saule_lexer::Token::LineComment`] / `BlockComment` into a [`Comment`]
//! and pass it in.
//!
//! Interleaving is best-effort but covers the common shapes:
//!
//! * Comments before a statement / declaration are emitted on their own
//!   line at the surrounding indent.
//! * A comment that sits on the same source line as the statement it
//!   trails is re-emitted as a same-line trailing comment.
//! * Comments at the tail of a block (just before the closing `end`) are
//!   drained at the right indent so they don't leak past the block.
//! * Blank lines between source comments are preserved when ≥ 2 newlines
//!   separated them in the original source.
//!
//! ## Indentation
//!
//! [`FmtOptions`] carries the indent unit and width. A project can declare
//! them once in its `saule.config` — see [`config`] for the keys and for the
//! precedence between that file, an editor's LSP options, and `saule fmt`'s
//! own flags.

mod decls;
mod exprs;
mod output;
mod stmts;

use std::{collections::VecDeque, ops::Range};

pub mod config;

pub use config::ConfigIndent;

use saule_ast::{BinOp, Decl, Expr, Module, Param, Spanned, Stmt, TableEntry, Type};

/// A single source comment extracted from the lexer's trivia stream.
/// `text` is the verbatim payload between the comment delimiters (no
/// `--` / `--[[` / `]]`), matching what `tokenize_with_trivia` emits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub span: Range<usize>,
    pub kind: CommentKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentKind {
    /// `-- text` to end of line.
    Line,
    /// `--[[ text ]]` (may span multiple source lines).
    Block,
}

/// Render a parsed module back to source, dropping any comments. Use
/// [`format_module_with_comments`] to preserve them. The result always
/// ends with exactly one trailing newline (or is empty for an empty
/// module).
pub fn format_module(module: &Module) -> String {
    let mut p = Printer::new("", &[], FmtOptions::default());
    p.module(module);
    p.finish()
}

/// Like [`format_module`] but threads `comments` (sorted or not, by span
/// start) back into the output. `source` is the original text, used to
/// tell same-line trailing comments from leading ones and to preserve
/// blank-line gaps.
pub fn format_module_with_comments(module: &Module, source: &str, comments: &[Comment]) -> String {
    format_module_with_options(module, source, comments, FmtOptions::default())
}

/// Like [`format_module_with_comments`] but with an explicit layout
/// configuration.
///
/// This is what the language server calls, mapping the editor's LSP
/// `FormattingOptions` (`tabSize` / `insertSpaces`) onto [`FmtOptions`] — so an
/// IDE's Code Style page actually drives the output instead of being ignored.
pub fn format_module_with_options(
    module: &Module,
    source: &str,
    comments: &[Comment],
    options: FmtOptions,
) -> String {
    let mut p = Printer::new(source, comments, options);
    p.module(module);
    p.finish()
}

/// Layout configuration for the printer.
///
/// [`Default`] is the canonical Saule style — 2-space indent, 100-column soft
/// target — and is what `saule fmt` uses when the caller has no opinion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FmtOptions {
    /// Columns per indent level. Clamped to `1..=16` on use, so a client
    /// sending `0` can't produce unindented output.
    pub indent_width: usize,
    /// Indent with hard tabs instead of spaces. `indent_width` still
    /// describes how wide one level *displays*, which is what the width
    /// calculations need.
    pub use_tabs: bool,
    /// Soft target for one rendered line; layouts that can break (table
    /// literals, pipelines, argument lists) flip to multi-line past it.
    pub max_width: usize,
}

impl Default for FmtOptions {
    fn default() -> Self {
        FmtOptions {
            indent_width: 2,
            use_tabs: false,
            max_width: 100,
        }
    }
}

impl FmtOptions {
    /// One indent level as it is written to the output.
    fn unit(&self) -> String {
        if self.use_tabs {
            "\t".to_string()
        } else {
            " ".repeat(self.indent_width.clamp(1, 16))
        }
    }

    /// Display width of one indent level, used for column arithmetic. A hard
    /// tab is counted as `indent_width` columns.
    fn display_width(&self) -> usize {
        self.indent_width.clamp(1, 16)
    }
}

struct Printer<'a> {
    out: String,
    indent: usize,
    /// Set right after a newline so the next `write_str` knows to prepend
    /// the current indentation. Avoids trailing whitespace on blank lines.
    needs_indent: bool,
    /// Original source text. Only consulted for newline-counting between
    /// byte positions when interleaving comments; empty when comments are
    /// disabled.
    source: &'a str,
    /// Pending comments, ordered by `span.start`. Drained as the printer
    /// reaches the corresponding source positions.
    comments: VecDeque<&'a Comment>,
    /// Highest source offset we've "consumed" so far — either the end of
    /// the last comment we drained, or 0. Used for blank-line preservation
    /// between consecutive comments.
    last_pos: usize,
    /// End offset of the most recently drained leading comment. Unlike
    /// `last_pos` this is never advanced past the comment, so it can measure
    /// the gap the author left between a comment and the code below it.
    last_comment_end: usize,
    /// Layout configuration: indent unit and the soft line-width target.
    opts: FmtOptions,
    /// `opts.unit()`, precomputed — it is written on every indented line.
    indent_unit: String,
    /// Set on measurement sub-printers: every breakable construct must render
    /// on one line.
    ///
    /// Without this a nested table or pipeline could emit newlines *into* the
    /// candidate string being measured, so the caller would compare a
    /// multi-line blob against the width budget and could then splice those
    /// newlines back in as if they were an inline form.
    force_inline: bool,
}

// ---- precedence / formatting helpers ---------------------------------------

/// Higher than every `bin_prec`, used as the lower bound for operands of
/// unary / postfix expressions so they always parenthesize inner binaries.
const MAX_PREC: u8 = 100;

/// Binding strength of `x as T` — above every binary operator (max 6) and
/// below the postfix chain, mirroring where `cast_expr` sits in the
/// parser's precedence ladder.
const CAST_PREC: u8 = MAX_PREC - 1;

/// (precedence, right_associative) for each binary operator, mirroring the
/// parser's Pratt table closely enough that re-parsing produces the same
/// tree.
fn bin_prec(op: BinOp) -> (u8, bool) {
    match op {
        BinOp::Or | BinOp::Coalesce => (1, false),
        BinOp::And => (2, false),
        BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => (3, false),
        BinOp::Concat => (4, true),
        BinOp::Add | BinOp::Sub => (5, false),
        BinOp::Mul | BinOp::Div | BinOp::Mod => (6, false),
        // `^` binds tighter than unary minus and is right-associative.
        BinOp::Pow => (7, true),
    }
}

fn bin_sym(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::Pow => "^",
        BinOp::Eq => "==",
        BinOp::NotEq => "!=",
        BinOp::Lt => "<",
        BinOp::LtEq => "<=",
        BinOp::Gt => ">",
        BinOp::GtEq => ">=",
        BinOp::And => "and",
        BinOp::Or => "or",
        BinOp::Concat => "..",
        BinOp::Coalesce => "??",
    }
}

/// Whether `raw` is a Saule float literal that reads back as exactly `f`.
///
/// Guards the verbatim path in [`Printer::float_lit`] against spans that
/// don't line up with the text we were handed — a synthesised AST, or a
/// `source` that isn't what the module was parsed from. Accepts the same
/// shapes the lexer does: digits with an optional single `.` (either side
/// may be empty, but not both), an optional `f`/`F` suffix, and the leading
/// `-` that negative literal *patterns* fold into their span.
fn float_text_matches(raw: &str, f: f64) -> bool {
    let body = raw
        .strip_suffix('f')
        .or_else(|| raw.strip_suffix('F'))
        .unwrap_or(raw);
    let digits = body.strip_prefix('-').unwrap_or(body);
    let well_formed = digits.chars().any(|c| c.is_ascii_digit())
        && digits.chars().all(|c| c.is_ascii_digit() || c == '.')
        && digits.chars().filter(|&c| c == '.').count() <= 1;
    well_formed && matches!(body.parse::<f64>(), Ok(v) if v == f)
}

/// Render an `f64` so it always reads back as a float (i.e. `1.0` rather
/// than `1`) and round-trips through `parse::<f64>()` losslessly for
/// finite values.
///
/// Only a fallback: [`Printer::float_lit`] prefers the author's own text.
fn format_float(f: f64) -> String {
    if !f.is_finite() {
        // Saule has no syntax for these; keep something readable.
        return format!("{f}");
    }
    let s = format!("{f}");
    if s.contains('.') || s.contains('e') || s.contains('E') {
        s
    } else {
        format!("{s}.0")
    }
}

fn quote_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Whether a single-param lambda came from the `name => expr` shortcut
/// (no type annotation, no return type, default-`any`). Matches the
/// parser's reconstruction so re-parsing produces the same AST.
fn is_bare_arrow_param(params: &[Param], return_ty: &Option<Type>) -> bool {
    if return_ty.is_some() || params.len() != 1 {
        return false;
    }
    let p = &params[0];
    !p.variadic && p.default.is_none() && matches!(&p.ty, Type::Named(n) if n == "any")
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Byte offset where a table entry starts in the source — used by the
/// formatter to detect a user-introduced line break between two entries
/// so the multi-line layout sticks.
fn entry_start(entry: &TableEntry) -> usize {
    match entry {
        TableEntry::Positional(e) => e.span.start,
        TableEntry::Field { key, .. } => key.span.start,
    }
}

/// Byte offset where a table entry ends in the source.
fn entry_end(entry: &TableEntry) -> usize {
    match entry {
        TableEntry::Positional(e) => e.span.end,
        TableEntry::Field { value, .. } => value.span.end,
    }
}

/// Whether two adjacent top-level statements should be separated by a
/// blank line. Declarations get breathing room; tight runs of locals or
/// expression statements stay compact. Consecutive `import` statements
/// are an exception — they read as a single block and stay packed.
fn needs_top_separator(prev: &Stmt, next: &Stmt) -> bool {
    let p_is_import = matches!(prev, Stmt::Decl(d) if matches!(d.value, Decl::Import { .. }));
    let n_is_import = matches!(next, Stmt::Decl(d) if matches!(d.value, Decl::Import { .. }));
    if p_is_import && n_is_import {
        return false;
    }
    let p_is_decl = matches!(prev, Stmt::Decl(_));
    let n_is_decl = matches!(next, Stmt::Decl(_));
    p_is_decl || n_is_decl
}

/// Byte offset of the first character on the line that contains `pos`.
/// Walks backwards in `source` to find the previous `\n`; returns
/// `pos` itself when out of range. Used at block entry to anchor
/// `last_pos` so a comment placed right under a header doesn't get
/// charged for the newlines above the header.
fn line_start_in_source(source: &str, pos: usize) -> usize {
    if pos > source.len() {
        return source.len();
    }
    source[..pos].rfind('\n').map(|n| n + 1).unwrap_or(0)
}

/// The byte offset where the next chunk of an `if … elseif … else … end`
/// starts. Used as the body-block ceiling when draining comments so they
/// don't escape past the `elseif` / `else` keyword.
fn next_if_chunk_start(
    remaining_elseifs: &[(Spanned<Expr>, Vec<Spanned<Stmt>>)],
    else_block: &Option<Vec<Spanned<Stmt>>>,
    fallback: usize,
) -> usize {
    if let Some((cond, _)) = remaining_elseifs.first() {
        return cond.span.start;
    }
    if let Some(eb) = else_block
        && let Some(first) = eb.first()
    {
        return first.span.start;
    }
    fallback
}
