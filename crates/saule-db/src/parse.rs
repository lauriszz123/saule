//! Getting a tree out of a buffer that is, most of the time, halfway
//! through an edit — and pulling out of it the one part that changes far
//! less often than the rest.

use saule_ast::{Decl, ImportNames, Module, Stmt};
use saule_lexer::LexerError;
use saule_parser::ParseError;

/// A parsed file: the tree, plus everything that was wrong with it in
/// pipeline order.
///
/// Two error lists rather than one interleaved by position: a lexical error
/// changes what the tokens *are*, so the parse errors below it are
/// downstream of it, and reporting them in that order is reporting them in
/// the order they need to be fixed.
pub struct Parsed {
    pub module: Module,
    pub lex: Vec<LexerError>,
    pub parse: Vec<ParseError>,
}

impl Parsed {
    /// Whether the file parsed without recovery — the precondition for
    /// anything that must not act on a guess, like publishing type
    /// diagnostics or rewriting the file.
    pub fn is_clean(&self) -> bool {
        self.lex.is_empty() && self.parse.is_empty()
    }
}

/// Lex and parse `source`, recovering at both stages, told where this
/// file's declarations lived at its last clean parse.
///
/// Total: there is no input this returns nothing for. The strict parser
/// answers "no tree" for any file with a syntax error, which describes a
/// source file for most of the time anyone is typing in it.
pub(crate) fn analyze(source: &str, prior: Option<&saule_parser::PriorShape>) -> Parsed {
    let lexed = saule_lexer::Lexer::new(source).tokenize_recover();
    let parsed = saule_parser::parse_recover_with_prior(lexed.tokens, source, prior);
    Parsed {
        module: parsed.module,
        lex: lexed.errors,
        parse: parsed.errors,
    }
}

/// A file's `import` statements, and a comparable summary of them.
///
/// The summary is what makes the import seed survive typing. Two `Imports`
/// are equal when they would drive the same walk over the same files —
/// spans, and everything else in the file, are deliberately absent.
pub struct Imports {
    /// The import statements, as a module of their own, ready to hand to
    /// the seed walk. Not part of equality: it carries spans, which move
    /// whenever anything above them is edited.
    pub module: Module,
    /// What the walk actually depends on: for each import, the names bound
    /// (with aliases, which decide what the seed is keyed under) and the
    /// path they come from.
    pub key: Vec<(String, String)>,
}

impl Imports {
    pub(crate) fn of(module: &Module) -> Imports {
        let mut stmts = Vec::new();
        let mut key = Vec::new();
        for stmt in &module.stmts {
            let Stmt::Decl(d) = &stmt.value else { continue };
            let Decl::Import { names, path, .. } = &d.value else {
                continue;
            };
            key.push((render_names(names), path.clone()));
            stmts.push(stmt.clone());
        }
        Imports {
            module: Module { stmts },
            key,
        }
    }
}

fn render_names(names: &ImportNames) -> String {
    match names {
        ImportNames::All => "*".to_string(),
        ImportNames::List(items) => items
            .iter()
            .map(|(orig, alias)| match alias {
                Some(a) => format!("{orig} as {a}"),
                None => orig.clone(),
            })
            .collect::<Vec<_>>()
            .join(","),
    }
}
