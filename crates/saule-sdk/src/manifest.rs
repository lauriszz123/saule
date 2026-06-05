//! Manifest registry — the single source of truth for a package's manifest.
//!
//! The package and its classes are registered with [`saule_package!`]; each
//! exported method is registered by the `#[saule_export]` attribute. [`render`]
//! walks the collected entries and produces the `<package>.toml` manifest the
//! interpreter reads from `~/.saule/native_manifests/`.
//!
//! Collection uses the `inventory` crate: every `submit!` places a `'static`
//! record into a program-global slice that [`render`] enumerates.
//!
//! [`saule_package!`]: crate::saule_package

use std::collections::BTreeMap;

/// Package-level metadata. Exactly one is registered, by [`saule_package!`].
///
/// [`saule_package!`]: crate::saule_package
pub struct PackageInfo {
    /// Import name, e.g. `"engine"`.
    pub name: &'static str,
    pub version: &'static str,
    /// Candidate binary filenames (`.so` / `.dll` / `.dylib`). The
    /// interpreter picks the one matching the host OS.
    pub binary: &'static [&'static str],
}

/// A class (static module) exposed by the package, with its doc string.
pub struct ExportedClass {
    pub name: &'static str,
    pub doc: &'static str,
}

/// One exported method, recorded by `#[saule_export]`.
pub struct ExportedMethod {
    /// Owning class, e.g. `"Window"`.
    pub class: &'static str,
    /// Saule-visible method name, e.g. `"create"`.
    pub name: &'static str,
    /// Saule type signature, inferred from the Rust function.
    pub sig: &'static str,
    /// The exported C symbol the manifest binds to.
    pub symbol: &'static str,
}

inventory::collect!(PackageInfo);
inventory::collect!(ExportedClass);
inventory::collect!(ExportedMethod);

/// Render the package manifest TOML from the registered exports.
///
/// Classes and methods are sorted by name so the output is deterministic.
/// Returns an error string if no package was registered.
pub fn render() -> Result<String, String> {
    let pkg = inventory::iter::<PackageInfo>
        .into_iter()
        .next()
        .ok_or("no package registered — call saule_package! in your crate root")?;

    let mut classes: Vec<&ExportedClass> = inventory::iter::<ExportedClass>.into_iter().collect();
    classes.sort_by_key(|c| c.name);

    let mut methods_by_class: BTreeMap<&str, Vec<&ExportedMethod>> = BTreeMap::new();
    for m in inventory::iter::<ExportedMethod> {
        methods_by_class.entry(m.class).or_default().push(m);
    }
    for methods in methods_by_class.values_mut() {
        methods.sort_by_key(|m| m.name);
    }

    let mut out = String::new();
    out.push_str("# AUTO-GENERATED from #[saule_export] declarations — do not edit by hand.\n");
    out.push_str("# Edit the Rust source and regenerate with the package's gen-manifest binary.\n\n");

    out.push_str("[package]\n");
    out.push_str(&format!("name = {}\n", toml_string(pkg.name)));
    out.push_str(&format!("version = {}\n", toml_string(pkg.version)));
    out.push_str(&format!("binary = {}\n", toml_string(&pkg.binary.join(", "))));

    for class in &classes {
        out.push('\n');
        out.push_str(&format!("[exports.{}]\n", class.name));
        out.push_str("type = \"class\"\n");
        out.push_str(&format!("doc = {}\n", toml_string(class.doc)));

        if let Some(methods) = methods_by_class.get(class.name) {
            for m in methods {
                out.push('\n');
                out.push_str(&format!("[[exports.{}.methods]]\n", class.name));
                out.push_str(&format!("name = {}\n", toml_string(m.name)));
                out.push_str(&format!("sig = {}\n", toml_string(m.sig)));
                out.push_str(&format!("native_symbol = {}\n", toml_string(m.symbol)));
            }
        }
    }

    Ok(out)
}

/// Encode `s` as a TOML basic string with the minimal escaping our values
/// need (quotes, backslashes, control chars).
fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}
