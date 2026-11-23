//! Where a built app's bundled files and native packages go on disk.
//!
//! A native package is a shared library, and the OS only loads one from a
//! file; assets are read by path (`Graphics.newImage`, `Io.open`). So both
//! are unpacked once, into a directory named after the app and its archive
//! hash, and every later start finds it complete and goes straight on.
//!
//! ```text
//! <cache>/saule/apps/<name>-<hash>/
//!   files/            the bundled files — Project.root
//!   native_packages/  the libraries — the app's Saule home
//! ```
//!
//! `<cache>` is `$XDG_CACHE_HOME` or `~/.cache` on Linux, `~/Library/Caches`
//! on macOS and `%LOCALAPPDATA%` on Windows. A directory is unpacked under a
//! temporary name and renamed into place, so a start that is interrupted,
//! or two starting at once, never leave a half-written one behind.

use std::path::PathBuf;

use super::archive::{Embedded, FILES, NATIVE};

/// Unpack what `embedded` carries for the disk, returning the directory —
/// or `None` when there is nothing to unpack.
pub fn unpack(name: &str, embedded: &Embedded) -> std::io::Result<Option<PathBuf>> {
    let wanted: Vec<_> = embedded
        .entries
        .iter()
        .filter(|e| e.path.starts_with(FILES) || e.path.starts_with(NATIVE))
        .collect();
    if wanted.is_empty() {
        return Ok(None);
    }

    let dir = cache_root().join("saule").join("apps").join(format!(
        "{}-{:016x}",
        safe(name),
        embedded.hash
    ));
    if dir.is_dir() {
        return Ok(Some(dir));
    }

    let parent = dir.parent().expect("the apps folder");
    std::fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".unpacking-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);

    for entry in wanted {
        let path = tmp.join(relative(&entry.path)?);
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        std::fs::write(&path, &entry.data)?;
    }
    // `native_packages/` must exist even when empty: it is what the loader
    // scans.
    std::fs::create_dir_all(tmp.join(NATIVE))?;

    match std::fs::rename(&tmp, &dir) {
        Ok(()) => Ok(Some(dir)),
        // Another start of the same app got there first.
        Err(_) if dir.is_dir() => {
            let _ = std::fs::remove_dir_all(&tmp);
            Ok(Some(dir))
        }
        Err(e) => Err(e),
    }
}

/// An archive path as a relative path, refusing one that would climb out of
/// the directory it is unpacked into.
fn relative(path: &str) -> std::io::Result<PathBuf> {
    let mut out = PathBuf::new();
    for part in path.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.contains('\\') {
            return Err(std::io::Error::other(format!(
                "unsafe path `{path}` in the archive"
            )));
        }
        out.push(part);
    }
    Ok(out)
}

fn safe(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "app".into()
    } else {
        cleaned
    }
}

fn cache_root() -> PathBuf {
    let env = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if cfg!(windows) {
        if let Some(dir) = env("LOCALAPPDATA") {
            return dir;
        }
    } else if cfg!(target_os = "macos") {
        if let Some(home) = env("HOME") {
            return home.join("Library").join("Caches");
        }
    } else if let Some(dir) = env("XDG_CACHE_HOME") {
        return dir;
    }
    env("HOME")
        .or_else(|| env("USERPROFILE"))
        .map(|home| home.join(".cache"))
        .unwrap_or_else(std::env::temp_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn archive_paths_may_not_leave_the_directory() {
        assert_eq!(
            relative("files/a/b.png").unwrap(),
            Path::new("files").join("a").join("b.png")
        );
        for bad in ["../x", "files/../../x", "/abs", "files//x", "a\\b"] {
            assert!(relative(bad).is_err(), "{bad}");
        }
    }
}
