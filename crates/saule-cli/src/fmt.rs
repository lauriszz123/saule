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

use miette::{NamedSource, Report};
use saule_fmt::{Comment, CommentKind, ConfigIndent, FmtOptions};
use saule_lexer::Token;

/// Everything the command line said, after parsing.
#[derive(Debug, Default, PartialEq, Eq)]
struct FmtArgs {
    /// `--write`/`-w`: overwrite in place instead of printing to stdout.
    write: bool,
    paths: Vec<PathBuf>,
    /// `--indent` / `--tabs` / `--spaces`, layered over whatever the
    /// project's `saule.config` said.
    overrides: ConfigIndent,
}

pub(crate) fn cmd_fmt(args: &[String]) {
    let parsed = match parse_args(args) {
        Ok(Some(parsed)) => parsed,
        // `--help` — already printed, nothing to format.
        Ok(None) => return,
        Err(msg) => {
            eprintln!("error: {msg}\n\n{HELP}");
            process::exit(2);
        }
    };

    let mut configs = ConfigCache::default();
    let mut had_error = false;
    for path in &parsed.paths {
        let options = parsed.overrides.apply_to(configs.options_for(path));
        if let Err(()) = fmt_one(path, parsed.write, options) {
            had_error = true;
        }
    }
    if had_error {
        process::exit(1);
    }
}

/// Tiny flag parser — no `--` separator support; fmt has no pass-through
/// args. Returns `Ok(None)` when `--help` was handled, `Err` with a
/// user-facing message otherwise.
fn parse_args(args: &[String]) -> Result<Option<FmtArgs>, String> {
    let mut out = FmtArgs::default();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "-w" | "--write" => out.write = true,
            "-h" | "--help" => {
                println!("{HELP}");
                return Ok(None);
            }
            "--tabs" => out.overrides.use_tabs = Some(true),
            "--spaces" => out.overrides.use_tabs = Some(false),
            // `--indent 4` and `--indent=4` are both accepted; the value is
            // the printer's own 1..=16 range, so a typo can't silently
            // produce unindented or absurdly indented output.
            "--indent" => {
                let value = rest
                    .next()
                    .ok_or_else(|| "`--indent` needs a number, e.g. `--indent 4`".to_string())?;
                out.overrides.indent_width = Some(parse_indent_width(value)?);
            }
            other if other.starts_with("--indent=") => {
                let value = other.trim_start_matches("--indent=");
                out.overrides.indent_width = Some(parse_indent_width(value)?);
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown fmt flag `{other}`"));
            }
            other => out.paths.push(PathBuf::from(other)),
        }
    }

    if out.paths.is_empty() {
        return Err("`fmt` needs at least one file path".to_string());
    }
    Ok(Some(out))
}

fn parse_indent_width(raw: &str) -> Result<usize, String> {
    match raw.parse::<usize>() {
        Ok(n) if (1..=16).contains(&n) => Ok(n),
        _ => Err(format!("`--indent {raw}` is not a number from 1 to 16")),
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

fn format_source(name: &str, source: &str, options: FmtOptions) -> Result<String, Report> {
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
    Ok(saule_fmt::format_module_with_options(
        &module, source, &comments, options,
    ))
}

const HELP: &str = "\
Usage:
  saule fmt <file.sau> [more.sau ...]      print formatted source to stdout
  saule fmt -w <file.sau> [more.sau ...]   overwrite files in place

Options:
  -w, --write       overwrite the files in place instead of printing
      --indent <n>  columns per indent level (1-16)
      --tabs        indent with hard tabs
      --spaces      indent with spaces
  -h, --help        show this help

Indentation defaults to the canonical 2 spaces, or to whatever the nearest
`saule.config` declares:

  indent_style: \"tab\"    -- or \"space\"
  indent_width: 4

The flags above override the config file for this run.";

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn parse(list: &[&str]) -> FmtArgs {
        parse_args(&args(list))
            .expect("args should parse")
            .expect("not a --help run")
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
        assert!(parsed.overrides.is_empty());
    }

    #[test]
    fn indent_flag_accepts_both_spellings() {
        assert_eq!(
            parse(&["--indent", "4", "a.sau"]).overrides.indent_width,
            Some(4)
        );
        assert_eq!(
            parse(&["--indent=8", "a.sau"]).overrides.indent_width,
            Some(8)
        );
    }

    #[test]
    fn tabs_and_spaces_flags_set_the_style() {
        assert_eq!(parse(&["--tabs", "a.sau"]).overrides.use_tabs, Some(true));
        assert_eq!(
            parse(&["--spaces", "a.sau"]).overrides.use_tabs,
            Some(false)
        );
        // Last one wins, so a wrapper script can append an override.
        assert_eq!(
            parse(&["--tabs", "--spaces", "a.sau"]).overrides.use_tabs,
            Some(false)
        );
    }

    #[test]
    fn bad_indent_values_are_rejected() {
        for bad in [
            args(&["--indent", "0", "a.sau"]),
            args(&["--indent", "99", "a.sau"]),
            args(&["--indent", "wide", "a.sau"]),
            args(&["--indent=x", "a.sau"]),
            // Missing value: nothing follows the flag.
            args(&["--indent"]),
        ] {
            assert!(parse_args(&bad).is_err(), "expected an error for {bad:?}");
        }
    }

    #[test]
    fn unknown_flags_and_empty_paths_are_rejected() {
        assert!(parse_args(&args(&["--nope", "a.sau"])).is_err());
        assert!(parse_args(&args(&["-w"])).is_err());
    }

    #[test]
    fn help_short_circuits() {
        assert_eq!(parse_args(&args(&["--help"])), Ok(None));
    }

    #[test]
    fn default_options_still_emit_two_spaces() {
        let out = format(SRC, FmtOptions::default());
        assert!(out.contains("\n  static fn main()"), "got:\n{out}");
    }

    #[test]
    fn flags_drive_the_printed_indent() {
        let parsed = parse(&["--indent", "4", "a.sau"]);
        let out = format(SRC, parsed.overrides.apply_to(FmtOptions::default()));
        assert!(out.contains("\n    static fn main()"), "got:\n{out}");

        let parsed = parse(&["--tabs", "a.sau"]);
        let out = format(SRC, parsed.overrides.apply_to(FmtOptions::default()));
        assert!(out.contains("\n\tstatic fn main()"), "got:\n{out}");
    }

    #[test]
    fn flags_win_over_the_project_config() {
        let config = ConfigIndent::parse("indent_style: \"tab\"\n");
        let from_config = config.apply_to(FmtOptions::default());
        assert!(from_config.use_tabs);

        let parsed = parse(&["--spaces", "--indent", "3", "a.sau"]);
        let out = format(SRC, parsed.overrides.apply_to(from_config));
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
