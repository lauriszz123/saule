//! `saule fmt` — format Saule source files in place or write to stdout.
//!
//! Pipeline: read source → lex with trivia → split tokens & comments →
//! parse → [`saule_fmt::format_module_with_options`] → either print to
//! stdout or overwrite the file. Comments are preserved best-effort
//! (leading + same-line trailing). Source-only errors from the lexer /
//! parser are reported via miette and exit non-zero; the formatter
//! itself never fails on a parsed module.
//!
//! Indentation comes from the nearest `saule.config` above each file
//! (`indent_style:` / `indent_width:`), overridden by `--indent` / `--tabs`
//! / `--spaces`, and defaults to the canonical 2 spaces when neither says
//! anything. See [`saule_fmt::config`] for the full precedence, including
//! how it interacts with the options an editor sends the language server.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    process,
};

use clap::Args;
use miette::{NamedSource, Report};
use saule_fmt::{Comment, CommentKind, ConfigIndent, FmtOptions};
use saule_lexer::Token;

/// `saule fmt [-w] [--indent N] [--tabs|--spaces] <file.sau>...`
#[derive(Debug, Args)]
#[command(
    about = "Format Saule source files",
    long_about = "\
Format Saule source files, printing to stdout or rewriting them in place.

Indentation defaults to the canonical 2 spaces, or to whatever the nearest
`saule.config` declares:

  indent_style: \"tab\"    -- or \"space\"
  indent_width: 4

The flags below override the config file for this run."
)]
pub(crate) struct FmtArgs {
    /// Files to format; at least one is required.
    #[arg(required = true, value_name = "FILE")]
    paths: Vec<PathBuf>,

    /// Overwrite the files in place instead of printing to stdout.
    #[arg(short, long)]
    write: bool,

    /// Columns per indent level (1-16).
    #[arg(long, value_name = "N", value_parser = parse_indent_width)]
    indent: Option<usize>,

    /// Indent with hard tabs.
    #[arg(long, overrides_with = "spaces")]
    tabs: bool,

    /// Indent with spaces.
    #[arg(long, overrides_with = "tabs")]
    spaces: bool,
}

impl FmtArgs {
    /// The `--indent` / `--tabs` / `--spaces` flags as an override layer to
    /// apply over whatever the project's `saule.config` said. `--tabs` and
    /// `--spaces` override each other, so the last one on the command line
    /// wins and a wrapper script can safely append one.
    fn overrides(&self) -> ConfigIndent {
        ConfigIndent {
            indent_width: self.indent,
            use_tabs: match (self.tabs, self.spaces) {
                (true, _) => Some(true),
                (_, true) => Some(false),
                _ => None,
            },
            warnings: Vec::new(),
        }
    }
}

pub(crate) fn cmd_fmt(args: &FmtArgs) {
    let overrides = args.overrides();
    let mut configs = ConfigCache::default();
    let mut had_error = false;
    for path in &args.paths {
        let options = overrides.apply_to(configs.options_for(path));
        if let Err(()) = fmt_one(path, args.write, options) {
            had_error = true;
        }
    }
    if had_error {
        process::exit(1);
    }
}

/// The printer's own range, enforced at parse time so a typo can't silently
/// produce unindented or absurdly indented output.
fn parse_indent_width(raw: &str) -> Result<usize, String> {
    match raw.parse::<usize>() {
        Ok(n) if (1..=16).contains(&n) => Ok(n),
        _ => Err(format!("`{raw}` is not a number from 1 to 16")),
    }
}

/// Remembers the `saule.config` lookup per directory so formatting a whole
/// project neither re-reads the same file once per source file nor repeats
/// its warnings.
#[derive(Default)]
struct ConfigCache {
    by_dir: HashMap<PathBuf, FmtOptions>,
    warned: HashSet<PathBuf>,
}

impl ConfigCache {
    /// Base options for `file`: the canonical style, with the nearest
    /// project's declared indentation layered on top.
    fn options_for(&mut self, file: &Path) -> FmtOptions {
        let dir = file.parent().unwrap_or(Path::new(".")).to_path_buf();
        if let Some(cached) = self.by_dir.get(&dir) {
            return *cached;
        }

        let mut options = FmtOptions::default();
        if let Some((config_path, indent)) = saule_fmt::config::load_project_indent(file) {
            if self.warned.insert(config_path.clone()) {
                for warning in &indent.warnings {
                    eprintln!("warning: {}: {warning}", config_path.display());
                }
            }
            options = indent.apply_to(options);
        }
        self.by_dir.insert(dir, options);
        options
    }
}

fn fmt_one(path: &PathBuf, write: bool, options: FmtOptions) -> Result<(), ()> {
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

    let formatted = match format_source(&name, &source, options) {
        Ok(s) => s,
        Err(report) => {
            eprintln!("{report:?}");
            return Err(());
        }
    };

    if write {
        if formatted != source
            && let Err(err) = fs::write(path, &formatted)
        {
            eprintln!("error writing file '{}': {}", path.display(), err);
            return Err(());
        }
    } else {
        print!("{formatted}");
    }
    Ok(())
}

fn format_source(name: &str, source: &str, options: FmtOptions) -> Result<String, Report> {
    let make_src = || NamedSource::new(name, source.to_string());
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
    Ok(saule_fmt::format_module_with_options(
        &module, source, &comments, options,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// Wrap `FmtArgs` on its own so the tests exercise the real derive
    /// without going through the whole `saule fmt …` command surface.
    #[derive(Debug, Parser)]
    struct Harness {
        #[command(flatten)]
        fmt: FmtArgs,
    }

    fn try_parse(list: &[&str]) -> Result<FmtArgs, clap::Error> {
        let argv = std::iter::once("fmt").chain(list.iter().copied());
        Harness::try_parse_from(argv).map(|h| h.fmt)
    }

    fn parse(list: &[&str]) -> FmtArgs {
        try_parse(list).expect("args should parse")
    }

    const SRC: &str = "class Main\n  static fn main()\n    println(\"hi\")\n  end\nend\n";

    fn format(src: &str, options: FmtOptions) -> String {
        format_source("test.sau", src, options).expect("corpus source parses")
    }

    #[test]
    fn paths_and_write_flag_parse() {
        let parsed = parse(&["-w", "a.sau", "b.sau"]);
        assert!(parsed.write);
        assert_eq!(
            parsed.paths,
            vec![PathBuf::from("a.sau"), PathBuf::from("b.sau")]
        );
        assert!(parsed.overrides().is_empty());
    }

    #[test]
    fn flags_may_follow_the_paths() {
        // clap is not positional about flags the way the old parser's loop
        // happened to be; `saule fmt a.sau -w` works.
        let parsed = parse(&["a.sau", "-w", "--indent", "4"]);
        assert!(parsed.write);
        assert_eq!(parsed.paths, vec![PathBuf::from("a.sau")]);
        assert_eq!(parsed.overrides().indent_width, Some(4));
    }

    #[test]
    fn indent_flag_accepts_both_spellings() {
        assert_eq!(
            parse(&["--indent", "4", "a.sau"]).overrides().indent_width,
            Some(4)
        );
        assert_eq!(
            parse(&["--indent=8", "a.sau"]).overrides().indent_width,
            Some(8)
        );
    }

    #[test]
    fn tabs_and_spaces_flags_set_the_style() {
        assert_eq!(parse(&["--tabs", "a.sau"]).overrides().use_tabs, Some(true));
        assert_eq!(
            parse(&["--spaces", "a.sau"]).overrides().use_tabs,
            Some(false)
        );
        // Last one wins, so a wrapper script can append an override.
        assert_eq!(
            parse(&["--tabs", "--spaces", "a.sau"]).overrides().use_tabs,
            Some(false)
        );
        assert_eq!(
            parse(&["--spaces", "--tabs", "a.sau"]).overrides().use_tabs,
            Some(true)
        );
    }

    #[test]
    fn bad_indent_values_are_rejected() {
        for bad in [
            vec!["--indent", "0", "a.sau"],
            vec!["--indent", "99", "a.sau"],
            vec!["--indent", "wide", "a.sau"],
            vec!["--indent=x", "a.sau"],
            // Missing value: nothing follows the flag.
            vec!["--indent"],
        ] {
            assert!(try_parse(&bad).is_err(), "expected an error for {bad:?}");
        }
    }

    #[test]
    fn unknown_flags_and_empty_paths_are_rejected() {
        assert!(try_parse(&["--nope", "a.sau"]).is_err());
        assert!(try_parse(&["-w"]).is_err());
        assert!(try_parse(&[]).is_err());
    }

    #[test]
    fn default_options_still_emit_two_spaces() {
        let out = format(SRC, FmtOptions::default());
        assert!(out.contains("\n  static fn main()"), "got:\n{out}");
    }

    #[test]
    fn flags_drive_the_printed_indent() {
        let parsed = parse(&["--indent", "4", "a.sau"]);
        let out = format(SRC, parsed.overrides().apply_to(FmtOptions::default()));
        assert!(out.contains("\n    static fn main()"), "got:\n{out}");

        let parsed = parse(&["--tabs", "a.sau"]);
        let out = format(SRC, parsed.overrides().apply_to(FmtOptions::default()));
        assert!(out.contains("\n\tstatic fn main()"), "got:\n{out}");
    }

    #[test]
    fn flags_win_over_the_project_config() {
        let config = ConfigIndent::parse("indent_style: \"tab\"\n");
        let from_config = config.apply_to(FmtOptions::default());
        assert!(from_config.use_tabs);

        let parsed = parse(&["--spaces", "--indent", "3", "a.sau"]);
        let out = format(SRC, parsed.overrides().apply_to(from_config));
        assert!(out.contains("\n   static fn main()"), "got:\n{out}");
    }

    #[test]
    fn config_is_read_from_the_nearest_ancestor() {
        // Two sibling projects with different styles, formatted in one run:
        // each file must follow the config above it, not the first one seen.
        let root = std::env::temp_dir().join(format!("saule-fmt-cfg-{}", std::process::id()));
        let tabs = root.join("tabbed/src");
        let spaces = root.join("spaced/src");
        fs::create_dir_all(&tabs).unwrap();
        fs::create_dir_all(&spaces).unwrap();
        fs::write(root.join("tabbed/saule.config"), "indent_style: \"tab\"\n").unwrap();
        fs::write(root.join("spaced/saule.config"), "indent_width: 4\n").unwrap();
        let tabbed_file = tabs.join("a.sau");
        let spaced_file = spaces.join("a.sau");
        fs::write(&tabbed_file, SRC).unwrap();
        fs::write(&spaced_file, SRC).unwrap();

        let mut cache = ConfigCache::default();
        let tabbed = cache.options_for(&tabbed_file);
        assert!(tabbed.use_tabs, "tabbed project should format with tabs");
        let spaced = cache.options_for(&spaced_file);
        assert!(!spaced.use_tabs);
        assert_eq!(spaced.indent_width, 4);

        // No config anywhere above → untouched defaults.
        let loose = std::env::temp_dir().join("saule-fmt-no-config-XXXX.sau");
        assert_eq!(
            ConfigCache::default().options_for(&loose),
            FmtOptions::default()
        );

        fs::remove_dir_all(&root).ok();
    }
}
