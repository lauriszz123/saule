//! `saule fmt` — format Saule source files in place or write to stdout.
//!
//! Pipeline: read source → lex with trivia → split tokens & comments →
//! parse → [`saule_fmt::format_module_with_comments`] → either print to
//! stdout or overwrite the file. Comments are preserved best-effort
//! (leading + same-line trailing). Source-only errors from the lexer /
//! parser are reported via miette and exit non-zero; the formatter
//! itself never fails on a parsed module.

use std::{fs, path::PathBuf, process};

use miette::{NamedSource, Report};
use saule_fmt::{Comment, CommentKind};
use saule_lexer::Token;

pub(crate) fn cmd_fmt(args: &[String]) {
    // Tiny flag parser — `--write`/`-w` overwrites files in place, anything
    // else is treated as a path. No `--` separator support; fmt has no
    // pass-through args.
    let mut write = false;
    let mut paths: Vec<PathBuf> = Vec::new();
    for a in args {
        match a.as_str() {
            "-w" | "--write" => write = true,
            "-h" | "--help" => {
                println!("{HELP}");
                return;
            }
            other if other.starts_with('-') => {
                eprintln!("error: unknown fmt flag `{other}`\n\n{HELP}");
                process::exit(2);
            }
            other => paths.push(PathBuf::from(other)),
        }
    }

    if paths.is_empty() {
        eprintln!("error: `fmt` needs at least one file path\n\n{HELP}");
        process::exit(2);
    }

    let mut had_error = false;
    for path in &paths {
        if let Err(()) = fmt_one(path, write) {
            had_error = true;
        }
    }
    if had_error {
        process::exit(1);
    }
}

fn fmt_one(path: &PathBuf, write: bool) -> Result<(), ()> {
    if !path.exists() {
        eprintln!("error: file '{}' does not exist", path.display());
        return Err(());
    }
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("error reading file '{}': {}", path.display(), err);
            return Err(());
        }
    };
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());

    let formatted = match format_source(&name, &source) {
        Ok(s) => s,
        Err(report) => {
            eprintln!("{report:?}");
            return Err(());
        }
    };

    if write {
        if formatted != source {
            if let Err(err) = fs::write(path, &formatted) {
                eprintln!("error writing file '{}': {}", path.display(), err);
                return Err(());
            }
        }
    } else {
        print!("{formatted}");
    }
    Ok(())
}

fn format_source(name: &str, source: &str) -> Result<String, Report> {
    let make_src = || NamedSource::new(name.to_string(), source.to_string());
    let raw = saule_lexer::Lexer::new(source)
        .tokenize_with_trivia()
        .map_err(|e| Report::new(e).with_source_code(make_src()))?;

    // Partition trivia comments off from the parser token stream.
    let mut comments: Vec<Comment> = Vec::new();
    let mut tokens = Vec::with_capacity(raw.len());
    for tok in raw {
        match tok.value {
            Token::LineComment(text) => comments.push(Comment {
                span: tok.span,
                kind: CommentKind::Line,
                text,
            }),
            Token::BlockComment(text) => comments.push(Comment {
                span: tok.span,
                kind: CommentKind::Block,
                text,
            }),
            other => tokens.push(saule_ast::Spanned {
                value: other,
                span: tok.span,
            }),
        }
    }

    let module =
        saule_parser::parse(tokens).map_err(|e| Report::new(e).with_source_code(make_src()))?;
    Ok(saule_fmt::format_module_with_comments(
        &module, source, &comments,
    ))
}

const HELP: &str = "\
Usage:
  saule fmt <file.sau> [more.sau ...]      print formatted source to stdout
  saule fmt -w <file.sau> [more.sau ...]   overwrite files in place";
