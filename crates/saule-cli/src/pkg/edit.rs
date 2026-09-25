//! Adding and removing `dependencies:` entries in a `saule.config`.
//!
//! The config is a file a person wrote — comments, key order, the two-space
//! indent they chose — so this rewrites the one line it must and leaves
//! every other byte alone. A round-trip through a parser and a printer would
//! be shorter and would quietly reformat somebody's file.

use saule_project::spec::Dependency;

/// The result of an edit: the new text, and whether anything changed.
pub(crate) struct Edited {
    pub(crate) text: String,
    pub(crate) changed: bool,
}

/// Add `entry` to `dependencies:`, replacing any entry for the same package.
///
/// Replacing rather than appending is what makes `saule install pkg@v2` after
/// `@v1` do the obvious thing instead of listing the package twice.
pub(crate) fn add_dependency(text: &str, entry: &str) -> Edited {
    let replacing = Dependency::parse(entry)
        .ok()
        .and_then(|d| d.as_package().cloned());

    let keep = |existing: &str| match (&replacing, Dependency::parse(existing)) {
        (Some(new), Ok(Dependency::Package(old))) => {
            !(old.host == new.host && old.owner == new.owner && old.repo == new.repo)
        }
        _ => existing.trim() != entry.trim(),
    };

    match find_list(text, "dependencies") {
        Some(found) => {
            let mut items: Vec<String> = found
                .items
                .iter()
                .filter(|e| keep(e))
                .map(|e| quote(e))
                .collect();
            items.push(quote(entry));
            // Whether anything changed is whether the file would change.
            // Deriving it from the edits instead gets the already-present
            // case wrong, which is the common one on a second `install`.
            let rewritten = replace_list(text, &found, &items);
            Edited {
                changed: rewritten != text,
                text: rewritten,
            }
        }
        // No `dependencies:` key at all: add one. At the end, where an
        // appended key reads as an addition rather than as a rewrite.
        None => {
            let mut out = text.to_string();
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&format!("dependencies: [{}]\n", quote(entry)));
            Edited {
                text: out,
                changed: true,
            }
        }
    }
}

/// Remove every `dependencies:` entry matching `needle` — the full entry,
/// `owner/repo`, the bare repo name, or a path written exactly as it appears.
///
/// Returns the entries removed, so the caller can report what it did and say
/// nothing matched when nothing did.
pub(crate) fn remove_dependency(text: &str, needle: &str) -> (Edited, Vec<String>) {
    let Some(found) = find_list(text, "dependencies") else {
        return (
            Edited {
                text: text.to_string(),
                changed: false,
            },
            Vec::new(),
        );
    };

    let matches = |entry: &str| match Dependency::parse(entry) {
        Ok(Dependency::Package(p)) => p.matches(needle),
        _ => entry.trim() == needle.trim(),
    };

    let removed: Vec<String> = found.items.iter().filter(|e| matches(e)).cloned().collect();
    if removed.is_empty() {
        return (
            Edited {
                text: text.to_string(),
                changed: false,
            },
            removed,
        );
    }
    let kept: Vec<String> = found
        .items
        .iter()
        .filter(|e| !matches(e))
        .map(|e| quote(e))
        .collect();
    (
        Edited {
            text: replace_list(text, &found, &kept),
            changed: true,
        },
        removed,
    )
}

/// A `key: [...]` line located in the text.
struct Found {
    /// Byte range of the whole line, without its newline.
    line: std::ops::Range<usize>,
    /// The indentation the line was written with, preserved on rewrite.
    indent: String,
    items: Vec<String>,
}

/// Find `key: [...]`, returning its span and current items.
///
/// Single-line only, which is the shape the format documents and every
/// config in the repository uses. A list broken across lines is left alone
/// rather than mangled — the caller reports it instead of half-editing it.
fn find_list(text: &str, key: &str) -> Option<Found> {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let len = line.len();
        let trimmed = line.trim_end_matches(['\n', '\r']);
        let body = trimmed.trim_start();
        if !body.starts_with("--")
            && let Some((k, value)) = body.split_once(':')
            && k.trim() == key
        {
            let value = value.trim();
            if value.starts_with('[') && value.ends_with(']') {
                let indent = trimmed[..trimmed.len() - body.len()].to_string();
                return Some(Found {
                    line: offset..offset + trimmed.len(),
                    indent,
                    items: split_items(&value[1..value.len() - 1]),
                });
            }
            return None;
        }
        offset += len;
    }
    None
}

/// Rewrite the located line with `items`.
fn replace_list(text: &str, found: &Found, items: &[String]) -> String {
    let line = format!("{}dependencies: [{}]", found.indent, items.join(", "));
    let mut out = String::with_capacity(text.len() + line.len());
    out.push_str(&text[..found.line.start]);
    out.push_str(&line);
    out.push_str(&text[found.line.end..]);
    out
}

/// Split a bracket's contents into entries, dropping the quotes.
fn split_items(inner: &str) -> Vec<String> {
    inner
        .split(',')
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn quote(entry: &str) -> String {
    format!("\"{}\"", entry.trim().trim_matches('"'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_is_appended_to_an_existing_list() {
        let out = add_dependency("dependencies: [\"../json\"]\n", "gh:o/r@v1");
        assert!(out.changed);
        assert_eq!(out.text, "dependencies: [\"../json\", \"gh:o/r@v1\"]\n");
    }

    #[test]
    fn a_missing_key_is_added() {
        let out = add_dependency("name: \"app\"\n", "gh:o/r@v1");
        assert_eq!(out.text, "name: \"app\"\ndependencies: [\"gh:o/r@v1\"]\n");
    }

    /// Installing a different ref of a package already listed replaces it,
    /// rather than listing the package twice at two versions.
    #[test]
    fn reinstalling_at_another_ref_replaces_the_entry() {
        let out = add_dependency("dependencies: [\"gh:o/r@v1\", \"../json\"]\n", "gh:o/r@v2");
        assert_eq!(out.text, "dependencies: [\"../json\", \"gh:o/r@v2\"]\n");
        assert!(out.changed);
    }

    #[test]
    fn adding_the_same_entry_twice_changes_nothing() {
        let first = add_dependency("dependencies: []\n", "gh:o/r@v1");
        let second = add_dependency(&first.text, "gh:o/r@v1");
        assert_eq!(second.text, first.text);
        assert!(!second.changed);
    }

    /// The file is somebody's: comments, key order and indentation survive.
    #[test]
    fn everything_but_the_one_line_is_untouched() {
        let before =
            "-- my app\nname: \"app\"\n\n  dependencies: [\"../json\"]\nentry: \"src/main.sau\"\n";
        let after = add_dependency(before, "gh:o/r@v1").text;
        assert_eq!(
            after,
            "-- my app\nname: \"app\"\n\n  dependencies: [\"../json\", \"gh:o/r@v1\"]\nentry: \"src/main.sau\"\n"
        );
    }

    #[test]
    fn removing_matches_by_slug_or_bare_name() {
        for needle in ["gh:o/r@v1", "o/r", "r"] {
            let (out, removed) =
                remove_dependency("dependencies: [\"../json\", \"gh:o/r@v1\"]\n", needle);
            assert_eq!(removed, ["gh:o/r@v1"], "needle `{needle}`");
            assert_eq!(out.text, "dependencies: [\"../json\"]\n");
        }
    }

    #[test]
    fn removing_a_path_needs_the_path() {
        let (out, removed) = remove_dependency("dependencies: [\"../json\"]\n", "../json");
        assert_eq!(removed, ["../json"]);
        assert_eq!(out.text, "dependencies: []\n");
    }

    #[test]
    fn removing_something_absent_reports_nothing_removed() {
        let (out, removed) = remove_dependency("dependencies: [\"../json\"]\n", "gh:o/r");
        assert!(removed.is_empty());
        assert!(!out.changed);
    }

    /// A list spread over several lines is not the documented shape, and
    /// half-rewriting it would be worse than declining.
    #[test]
    fn a_multiline_list_is_left_alone() {
        let text = "dependencies: [\n  \"../json\",\n]\n";
        assert!(find_list(text, "dependencies").is_none());
    }

    #[test]
    fn a_commented_out_key_is_not_the_key() {
        let text = "-- dependencies: [\"../old\"]\ndependencies: [\"../json\"]\n";
        let found = find_list(text, "dependencies").expect("the real key");
        assert_eq!(found.items, ["../json"]);
    }
}
