//! Project-mode bootstrap: read `saule.config`, configure the interpreter's
//! project context, then hand off to [`crate::run::run_file`] on the entry
//! point.

use std::{
    fs,
    path::{Path, PathBuf},
    process,
};

use crate::run::run_file;

pub(crate) fn run_project(dir: &Path) {
    let config_path = dir.join("saule.config");
    if !config_path.exists() {
        eprintln!(
            "error: no `saule.config` in `{}`\n\nRun `saule init <name>` to create one, or pass a file path.",
            dir.display()
        );
        process::exit(1);
    }

    let config = match read_config(&config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error reading {}: {e}", config_path.display());
            process::exit(1);
        }
    };

    // Canonicalise the project root so every `pretty_path` / `src_dirs`
    // comparison downstream is comparing apples to apples.
    let root = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());

    // min_saule_version: refuse to run on a stale toolchain.
    if let Some(min) = config.min_saule_version.as_deref() {
        let current = env!("CARGO_PKG_VERSION");
        if !version_at_least(current, min) {
            eprintln!("error: this project requires Saule {min} or newer (current: {current})");
            process::exit(1);
        }
    }

    let src_dirs: Vec<PathBuf> = config.src_dirs.iter().map(|s| root.join(s)).collect();

    saule_interpreter::project::set(saule_interpreter::project::ProjectInfo {
        name: config.name.clone().unwrap_or_default(),
        version: config.version.clone().unwrap_or_default(),
        root: root.clone(),
        src_dirs,
    });

    let entry_rel = config
        .entry
        .clone()
        .unwrap_or_else(|| "src/main.sau".to_string());
    let entry_path = root.join(&entry_rel);
    if !entry_path.is_file() {
        eprintln!(
            "error: entry `{entry_rel}` (from saule.config) does not exist at `{}`",
            entry_path.display()
        );
        process::exit(1);
    }

    run_file(entry_path, true);
}

/// Parsed `saule.config`. Unknown keys are silently dropped; the format is
/// deliberately minimal — `key: "value"` per line, plus `key: ["a", "b"]`
/// for list-valued keys, plus `--` line comments and blank lines.
#[derive(Debug, Default)]
struct RawConfig {
    name: Option<String>,
    version: Option<String>,
    entry: Option<String>,
    src_dirs: Vec<String>,
    min_saule_version: Option<String>,
}

fn read_config(path: &Path) -> std::io::Result<RawConfig> {
    let text = fs::read_to_string(path)?;
    let mut out = RawConfig::default();
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
            "src_dirs" => out.src_dirs = parse_list(value),
            "min_saule_version" => out.min_saule_version = Some(unquote(value)),
            _ => {}
        }
    }
    Ok(out)
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches('"').to_string()
}

/// Parse `["a", "b", "c"]` into `["a", "b", "c"]`. Tolerates missing
/// brackets (treats the value as a single entry) and stray whitespace.
fn parse_list(raw: &str) -> Vec<String> {
    let inner = raw.trim().trim_start_matches('[').trim_end_matches(']');
    inner
        .split(',')
        .map(|p| unquote(p.trim()))
        .filter(|s| !s.is_empty())
        .collect()
}

/// Numeric compare of dotted version strings (`"0.4.1" >= "0.4.0"`).
/// Non-numeric components compare as 0; missing components default to 0.
fn version_at_least(current: &str, required: &str) -> bool {
    let parse = |v: &str| -> Vec<u32> {
        v.split('.')
            .map(|p| {
                p.chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
            })
            .map(|p| p.parse::<u32>().unwrap_or(0))
            .collect()
    };
    let a = parse(current);
    let b = parse(required);
    let n = a.len().max(b.len());
    for i in 0..n {
        let ai = a.get(i).copied().unwrap_or(0);
        let bi = b.get(i).copied().unwrap_or(0);
        if ai != bi {
            return ai > bi;
        }
    }
    true
}
