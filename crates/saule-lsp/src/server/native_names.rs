//! Parameter-name table for stdlib `NativeSig`s.
//!
//! `saule_typeck::sigs::NativeSig` carries types but not names — names
//! are kept in the documentation, not the type registry — so for
//! signature help and inlay hints we maintain a small lookup table
//! here for the most-used stdlib functions and fall back to a
//! type-derived heuristic for everything else.
//!
//! When unknown, a name is synthesised from the parameter's type:
//! `string` → `s`, `integer` → `i`, `float`/`number` → `n`, and so on.
//! This is meaningfully better than `arg0` / `arg1` for unannotated
//! sigs, because the user actually sees what kind of value goes there.

use std::collections::HashMap;
use std::sync::OnceLock;

use saule_ast::Type;
use saule_typeck::sigs::NativeSig;

/// Lookup `(qname, sig)` and return one display name per parameter
/// slot. The returned vector always has length `sig.params.len() +
/// (sig.variadic.is_some() as usize)` — variadic gets its own
/// trailing slot named after the variadic type.
pub(super) fn param_names(qname: &str, sig: &NativeSig) -> Vec<String> {
    let table = stdlib_table();
    let mut out: Vec<String> = if let Some(named) = table.get(qname) {
        named.iter().map(|s| (*s).to_string()).collect()
    } else {
        derive_from_types(&sig.params)
    };
    // Make sure we always have enough slots; pad / clip to fit the
    // declared positional arity.
    out.truncate(sig.params.len());
    while out.len() < sig.params.len() {
        out.push(derive_one(&sig.params[out.len()], out.len()));
    }
    if sig.variadic.is_some() {
        let var_name = sig
            .variadic
            .as_ref()
            .map(|t| derive_one(t, out.len()))
            .unwrap_or_else(|| "rest".to_string());
        out.push(var_name);
    }
    // Disambiguate duplicates by appending an index — `s, s` becomes
    // `s, s2`. Common when a sig takes two strings (e.g. `Os.rename`).
    deduplicate(&mut out);
    out
}

/// Derive a name from a `Type`. Returns short, idiomatic identifiers
/// rather than raw type names — `string` becomes `s`, not `string` —
/// so the inlay rendering reads as code rather than a type ascription.
fn derive_one(ty: &Type, idx: usize) -> String {
    let base = match ty {
        Type::Named(n) => match n.as_str() {
            "string" => "s",
            "integer" => "i",
            "float" | "number" => "n",
            "boolean" => "flag",
            "any" => "value",
            other => {
                // PascalCase user/stdlib type → lowercased first
                // letter (`File` → `f`, `Player` → `p`). Keeps
                // names short and recognisable.
                if let Some(c) = other.chars().next() {
                    return format!("{}", c.to_ascii_lowercase());
                }
                "value"
            }
        },
        Type::Nullable(inner) => return derive_one(inner, idx),
        Type::Function { .. } => "fn",
        Type::Table { .. } => "t",
        Type::Tuple(_) => "tup",
    };
    base.to_string()
}

fn derive_from_types(types: &[Type]) -> Vec<String> {
    types
        .iter()
        .enumerate()
        .map(|(i, t)| derive_one(t, i))
        .collect()
}

fn deduplicate(names: &mut [String]) {
    let mut seen: HashMap<String, usize> = HashMap::new();
    for n in names.iter_mut() {
        let count = seen.entry(n.clone()).or_insert(0);
        *count += 1;
        if *count > 1 {
            *n = format!("{n}{count}");
        }
    }
}

/// Build (and cache) the static lookup table. Keep entries sorted by
/// module so it's easy to spot gaps when adding new stdlib calls.
fn stdlib_table() -> &'static HashMap<&'static str, &'static [&'static str]> {
    static TABLE: OnceLock<HashMap<&'static str, &'static [&'static str]>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut m: HashMap<&'static str, &'static [&'static str]> = HashMap::new();

        // ── Math ────────────────────────────────────────────────
        for f in &[
            "Math.floor", "Math.ceil", "Math.round", "Math.sign", "Math.sqrt",
            "Math.sin", "Math.cos", "Math.tan", "Math.asin", "Math.acos",
            "Math.exp", "Math.deg", "Math.rad", "Math.abs", "Math.type",
        ] {
            m.insert(f, &["n"]);
        }
        m.insert("Math.atan", &["y", "x"]);
        m.insert("Math.log", &["x", "base"]);
        m.insert("Math.max", &["a", "b"]);
        m.insert("Math.min", &["a", "b"]);
        m.insert("Math.pow", &["base", "exp"]);
        m.insert("Math.random", &["lo", "hi"]);
        m.insert("Math.randomseed", &["seed"]);
        m.insert("Math.modf", &["n"]);
        m.insert("Math.fmod", &["a", "b"]);

        // ── String ──────────────────────────────────────────────
        m.insert("String.byte", &["s", "i"]);
        m.insert("String.char", &["codepoint"]);
        m.insert("String.format", &["fmt"]);
        m.insert("String.len", &["s"]);
        m.insert("String.sub", &["s", "i", "j"]);
        m.insert("String.rep", &["s", "n"]);
        m.insert("String.starts", &["s", "prefix"]);
        m.insert("String.ends", &["s", "suffix"]);
        m.insert("String.find", &["s", "pattern", "init"]);
        m.insert("String.lower", &["s"]);
        m.insert("String.upper", &["s"]);
        m.insert("String.iter", &["s"]);
        m.insert("String.split", &["s", "sep"]);
        m.insert("String.trim", &["s"]);
        m.insert("String.replace", &["s", "from", "to"]);
        m.insert("String.contains", &["s", "needle"]);

        // ── Os ──────────────────────────────────────────────────
        m.insert("Os.difftime", &["t2", "t1"]);
        m.insert("Os.date", &["fmt", "time"]);
        m.insert("Os.sleep", &["seconds"]);
        m.insert("Os.getenv", &["name"]);
        m.insert("Os.setenv", &["name", "value"]);
        m.insert("Os.chdir", &["path"]);
        m.insert("Os.remove", &["path"]);
        m.insert("Os.rename", &["from", "to"]);
        m.insert("Os.list", &["path"]);
        m.insert("Os.exists", &["path"]);
        m.insert("Os.mkdir", &["path", "recursive"]);
        m.insert("Os.exit", &["code"]);
        m.insert("Os.execute", &["cmd"]);
        m.insert("Os.read_file", &["path"]);
        m.insert("Os.write_file", &["path", "contents"]);
        m.insert("Os.append_file", &["path", "contents"]);
        m.insert("Os.is_dir", &["path"]);
        m.insert("Os.is_file", &["path"]);

        // ── Io ──────────────────────────────────────────────────
        m.insert("Io.open", &["path", "mode"]);
        m.insert("Io.read", &["path"]);
        m.insert("Io.write", &["path", "contents"]);
        m.insert("File.read", &["mode"]);
        m.insert("File.write", &["data"]);
        m.insert("File.close", &[]);
        m.insert("File.lines", &[]);
        m.insert("File.seek", &["whence", "offset"]);

        // ── Table ───────────────────────────────────────────────
        m.insert("Table.insert", &["t", "value", "pos"]);
        m.insert("Table.remove", &["t", "pos"]);
        m.insert("Table.concat", &["t", "sep", "i", "j"]);
        m.insert("Table.sort", &["t", "cmp"]);
        m.insert("Table.unpack", &["t", "i", "j"]);
        m.insert("Table.pack", &["values"]);
        m.insert("Table.contains", &["t", "value"]);
        m.insert("Table.indexof", &["t", "value"]);
        m.insert("Table.copy", &["t"]);
        m.insert("Table.length", &["t"]);
        m.insert("Table.keys", &["t"]);
        m.insert("Table.values", &["t"]);

        // ── Iter ────────────────────────────────────────────────
        m.insert("Iter.range", &["from", "to", "step"]);
        m.insert("Iter.map", &["iter", "fn"]);
        m.insert("Iter.filter", &["iter", "predicate"]);
        m.insert("Iter.reduce", &["iter", "init", "fn"]);
        m.insert("Iter.take", &["iter", "n"]);
        m.insert("Iter.skip", &["iter", "n"]);
        m.insert("Iter.collect", &["iter"]);

        // ── core (bare) ─────────────────────────────────────────
        m.insert("println", &["value"]);
        m.insert("print", &["value"]);
        m.insert("printf", &["fmt"]);
        m.insert("assert", &["cond", "message"]);
        m.insert("error", &["message", "level"]);
        m.insert("typeof", &["value"]);
        m.insert("tostring", &["value"]);
        m.insert("tonumber", &["s", "base"]);
        m.insert("pairs", &["t"]);
        m.insert("ipairs", &["t"]);

        m
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use saule_typeck::sigs::NativeSig;

    fn sig(params: Vec<Type>, variadic: Option<Type>) -> NativeSig {
        NativeSig {
            type_params: vec![],
            params,
            variadic,
            returns: vec![],
        }
    }

    #[test]
    fn known_stdlib_returns_table_names() {
        let s = sig(
            vec![
                Type::Named("string".into()),
                Type::Named("string".into()),
                Type::Nullable(Box::new(Type::Named("integer".into()))),
            ],
            None,
        );
        assert_eq!(param_names("String.find", &s), vec!["s", "pattern", "init"]);
    }

    #[test]
    fn unknown_qname_falls_back_to_type_derived_names() {
        let s = sig(vec![Type::Named("string".into())], None);
        assert_eq!(param_names("Foo.bar", &s), vec!["s"]);
    }

    #[test]
    fn duplicate_type_derived_names_are_disambiguated() {
        let s = sig(
            vec![
                Type::Named("string".into()),
                Type::Named("string".into()),
                Type::Named("string".into()),
            ],
            None,
        );
        assert_eq!(param_names("Foo.bar", &s), vec!["s", "s2", "s3"]);
    }

    #[test]
    fn variadic_gets_its_own_named_slot() {
        let s = sig(
            vec![Type::Named("string".into())],
            Some(Type::Named("any".into())),
        );
        // `String.format` is in the table with one name; variadic
        // gets a synthesised `value` from `any`.
        let names = param_names("String.format", &s);
        assert_eq!(names.first().map(|s| s.as_str()), Some("fmt"));
        assert_eq!(names.len(), 2);
        assert_eq!(names[1], "value");
    }

    #[test]
    fn nullable_falls_through_to_inner_type() {
        let s = sig(
            vec![Type::Nullable(Box::new(Type::Named("integer".into())))],
            None,
        );
        assert_eq!(param_names("Foo.bar", &s), vec!["i"]);
    }
}
