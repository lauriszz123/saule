//! The `saule.config` format.
//!
//! Deliberately not TOML or YAML: one `key: "value"` per line, `key: ["a",
//! "b"]` for the two list-valued keys, `--` line comments, blank lines
//! ignored. Small enough that a hand-rolled parser is cheaper than a
//! dependency — but *only* if there is exactly one of them, which is what
//! this module is for.
//!
//! Unknown keys are dropped rather than rejected. That is a compatibility
//! promise as much as a parsing decision: a project written for a newer
//! toolchain still loads on an older one, minus the key it does not know.

use std::fs;
use std::path::Path;

/// A parsed `saule.config`.
///
/// Every field is optional at this layer. Defaulting is the caller's job
/// because the defaults differ by caller — `entry:` defaults to
/// `src/main.sau` only for something that is going to be *run*, and an
/// absent `src_dirs:` means "no extra import roots" to the resolver but
/// "look in `src/`" to a file walker. Baking either in here would force one
/// of those two readings on everyone.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Config {
    pub name: Option<String>,
    pub version: Option<String>,
    /// Path to the entry point, relative to the project root. Meaningless
    /// for a library.
    pub entry: Option<String>,
    /// `"app"` (the default) or `"library"`. Validate with [`Config::kind`].
    pub kind: Option<String>,
    /// Extra source roots, relative to the project root.
    pub src_dirs: Vec<String>,
    /// Minimum toolchain version. Compared by `saule_version::at_least`,
    /// which lives there so this check, `Saule.atLeast` in the language and
    /// the release tooling cannot drift apart.
    pub min_saule_version: Option<String>,
    /// Paths to other Saule projects: absolute, `~`-prefixed, or relative
    /// to this project's root. Resolved by [`crate::resolve_dependencies`].
    pub dependencies: Vec<String>,
}

/// What a project *is*, which decides whether it can be run at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Has an entry point and can be run.
    App,
    /// Exists to be imported. No entry point by definition.
    Library,
}

impl Config {
    /// Parse config text. Never fails — an unparseable line is skipped, in
    /// keeping with the unknown-key rule.
    pub fn parse(text: &str) -> Config {
        let mut out = Config::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with("--") {
                continue;
            }
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim();
            match key {
                "name" => out.name = Some(unquote(value)),
                "version" => out.version = Some(unquote(value)),
                "entry" => out.entry = Some(unquote(value)),
                "kind" => out.kind = Some(unquote(value)),
                "src_dirs" => out.src_dirs = parse_list(value),
                "min_saule_version" => out.min_saule_version = Some(unquote(value)),
                "dependencies" => out.dependencies = parse_list(value),
                _ => {}
            }
        }
        out
    }

    /// Read and parse the `saule.config` at `path`.
    pub fn read(path: &Path) -> std::io::Result<Config> {
        Ok(Config::parse(&fs::read_to_string(path)?))
    }

    /// Read the `saule.config` inside the directory `root`.
    pub fn read_in(root: &Path) -> std::io::Result<Config> {
        Config::read(&root.join(crate::CONFIG_FILE))
    }

    /// The declared [`Kind`], or the unrecognised spelling as an error so
    /// the caller can quote it back to the user.
    pub fn kind(&self) -> Result<Kind, &str> {
        match self.kind.as_deref() {
            None | Some("app") => Ok(Kind::App),
            Some("library") => Ok(Kind::Library),
            Some(other) => Err(other),
        }
    }

    /// `name:`, falling back to the project directory's own name so a config
    /// that omits it still yields a usable import prefix.
    pub fn name_or_dir(&self, root: &Path) -> String {
        self.name
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                root.file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
    }
}

/// Strip surrounding whitespace and quotes from a scalar value.
fn unquote(s: &str) -> String {
    s.trim().trim_matches('"').to_string()
}

/// Parse `["a", "b", "c"]`. Tolerates missing brackets (the whole value
/// becomes a single entry) and stray whitespace.
fn parse_list(raw: &str) -> Vec<String> {
    let inner = raw.trim().trim_start_matches('[').trim_end_matches(']');
    inner
        .split(',')
        .map(|p| unquote(p.trim()))
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_key() {
        let cfg = Config::parse(
            r#"
-- a comment
name: "demo"
version: "1.2.3"
entry: "src/app.sau"
kind: "library"
src_dirs: ["src", "vendor/src"]
min_saule_version: "26.4"
dependencies: ["../json", "~/libs/http"]
"#,
        );
        assert_eq!(cfg.name.as_deref(), Some("demo"));
        assert_eq!(cfg.version.as_deref(), Some("1.2.3"));
        assert_eq!(cfg.entry.as_deref(), Some("src/app.sau"));
        assert_eq!(cfg.kind(), Ok(Kind::Library));
        assert_eq!(cfg.src_dirs, ["src", "vendor/src"]);
        assert_eq!(cfg.min_saule_version.as_deref(), Some("26.4"));
        assert_eq!(cfg.dependencies, ["../json", "~/libs/http"]);
    }

    #[test]
    fn unknown_keys_and_junk_lines_are_ignored() {
        let cfg = Config::parse("name: \"x\"\nfuture_key: \"y\"\nnot a pair\n\n");
        assert_eq!(cfg.name.as_deref(), Some("x"));
        assert_eq!(cfg.kind(), Ok(Kind::App));
    }

    #[test]
    fn an_unknown_kind_is_reported_verbatim() {
        assert_eq!(Config::parse("kind: \"plugin\"").kind(), Err("plugin"));
    }

    #[test]
    fn a_list_without_brackets_is_one_entry() {
        assert_eq!(Config::parse("src_dirs: \"src\"").src_dirs, ["src"]);
    }

    #[test]
    fn name_falls_back_to_the_directory() {
        let cfg = Config::parse("name: \"\"");
        assert_eq!(cfg.name_or_dir(Path::new("/tmp/my-lib")), "my-lib");
    }
}
