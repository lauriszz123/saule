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
    // `DOCS.md` first: it documents every stdlib function with its parameter
    // names, so it covers the whole library rather than the most-used corner
    // of it, and it is the spelling the reader has already seen in the
    // reference. The table below predates it and stays as the fallback for a
    // name the documentation has not reached yet; `names_agree_with_the_docs`
    // keeps the two from drifting where both have an answer.
    let documented = saule_docs::stdlib()
        .entry(qname)
        .map(|e| e.params.clone())
        .filter(|p| !p.is_empty());
    let mut out: Vec<String> = if let Some(p) = documented {
        p
    } else if let Some(named) = table.get(qname) {
        named.iter().map(|s| (*s).to_string()).collect()
    } else {
        derive_from_types(&sig.params)
    };
    // Make sure we always have enough slots; pad / clip to fit the
    // declared positional arity.
    out.truncate(sig.params.len());
    while out.len() < sig.params.len() {
        out.push(derive_one(&sig.params[out.len()]));
    }
    if let Some(var_ty) = &sig.variadic {
        // The table may name the variadic slot too — `String.char`'s
        // `codepoint`, `Io.read`'s `path` — and that name is better than
        // anything derivable from `integer` or `string`. It sits at the index
        // just past the positional parameters. (The documentation never names
        // this slot: see `StdlibEntry::params`.)
        let var_name = table
            .get(qname)
            .and_then(|named| named.get(sig.params.len()))
            .map(|n| (*n).to_string())
            .unwrap_or_else(|| derive_one(var_ty));
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
fn derive_one(ty: &Type) -> String {
    let base = match ty {
        Type::Named(n) => match n.as_str() {
            "string" => "s",
            "integer" => "i",
            "float" => "n",
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
        Type::Nullable(inner) => return derive_one(inner),
        // Named after its head, as a bare name would be: `Box<Player>`
        // suggests `b`. The arguments say what is inside, not what to
        // call it.
        Type::Generic(g) => {
            return g
                .name
                .chars()
                .next()
                .map(|c| c.to_ascii_lowercase().to_string())
                .unwrap_or_else(|| "value".to_string());
        }
        Type::Function { .. } => "fn",
        Type::Table { .. } => "t",
        Type::Tuple(_) => "tup",
    };
    base.to_string()
}

fn derive_from_types(types: &[Type]) -> Vec<String> {
    types.iter().map(derive_one).collect()
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

        // What is left here after `DOCS.md` became the source of positional
        // names: the **variadic** slots, which the documentation declines to
        // name because `...string` declares a type and no name.
        //
        // This table used to carry every name for the most-used corner of the
        // library. Sixty-nine of those entries became unreachable the moment
        // `param_names` preferred the documentation, and eighteen of them
        // *contradicted* it — `String.find`'s second argument was called
        // `pattern` here, which was merely unhelpful before Lua patterns
        // existed and is actively wrong now that `String.gsub` takes a real
        // one. They are gone rather than left to rot:
        // `names_agree_with_the_docs` fails if a new one reappears.
        m.insert("String.char", &["codepoint"]);
        m.insert("String.format", &["fmt"]);
        m.insert("Io.read", &["path"]);
        m.insert("Io.write", &["path", "contents"]);
        m.insert("File.read", &["mode"]);
        m.insert("File.write", &["data"]);
        m.insert("Math.max", &["a"]);
        m.insert("Math.min", &["a"]);
        m.insert("Table.pack", &["values"]);
        m.insert("println", &["value"]);
        m.insert("print", &["value"]);
        m.insert("printf", &["fmt"]);

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
            bounds: Vec::new(),
            params,
            variadic,
            returns: vec![],
        }
    }

    #[test]
    fn known_stdlib_returns_documented_names() {
        let s = sig(
            vec![
                Type::Named("string".into()),
                Type::Named("string".into()),
                Type::Nullable(Box::new(Type::Named("integer".into()))),
            ],
            None,
        );
        // `needle` and `from`, not `pattern` and `init`: these come from
        // `DOCS.md` now, and the old hand-table spelling called a literal
        // substring a "pattern" — a name that was unhelpful before
        // `String.gsub` existed and wrong afterwards.
        assert_eq!(param_names("String.find", &s), vec!["s", "needle", "from"]);
    }

    /// A variadic slot is still named by the table, which is the only source
    /// with a name for it.
    #[test]
    fn a_variadic_slot_is_named_by_the_table() {
        let s = sig(vec![], Some(Type::Named("integer".into())));
        // `codepoint`, not the type-derived `i`.
        assert_eq!(param_names("String.char", &s), vec!["codepoint"]);
        let w = sig(vec![], Some(Type::Named("string".into())));
        assert_eq!(param_names("Io.read", &w), vec!["path"]);
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
    /// The hand table and `DOCS.md` must not disagree.
    ///
    /// Both answer "what are this function's parameters called", and the
    /// reader sees the documentation's spelling in the reference and the
    /// table's in signature help. Where both have an answer it has to be the
    /// same answer, or the editor contradicts the manual.
    ///
    /// A mismatch is fixed in whichever is wrong — usually the table, since
    /// the documentation's examples are executed by CI and its signature is
    /// the one checked against the compiler.
    #[test]
    fn every_registered_member_is_documented() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(saule_runtime::init);
        let docs = saule_docs::stdlib();
        let mut missing = Vec::new();
        let mut total = 0;
        for module in [
            "String", "Bytes", "Math", "Table", "Iter", "Io", "Os", "Saule", "Project", "File",
            "Process",
        ] {
            for m in saule_typeck::sigs::module_members(module) {
                total += 1;
                let q = format!("{module}.{m}");
                if docs.entry(&q).is_none() {
                    missing.push(q);
                }
            }
        }
        for free in [
            "print",
            "println",
            "printf",
            "tostring",
            "type",
            "assert",
            "error",
            "typeof",
            "ipairs",
            "pairs",
            "tonumber",
            "tointeger",
        ] {
            if saule_typeck::sigs::lookup(free).is_some() {
                total += 1;
                if docs.entry(free).is_none() {
                    missing.push(free.to_string());
                }
            }
        }
        missing.sort();
        assert!(
            missing.is_empty(),
            "{} of {total} registered stdlib members have no `DOCS.md` entry, so \
             they hover with types and no prose. Document them (a `####` \
             signature heading, or a row in the module's constant table):\n  {}",
            missing.len(),
            missing.join("\n  ")
        );
    }

    /// The standard library's *classes* are documented too.
    ///
    /// `Coroutine` and `Yielder` reach the checker as builtin classes rather
    /// than as native signatures, so the test above, which walks the
    /// signature registry, never sees their methods. Hover on `gen.resume`
    /// shows the `DOCS.md` entry for `Coroutine.resume`; a method added
    /// without one would hover as a bare signature.
    #[test]
    fn every_builtin_class_method_is_documented() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(saule_runtime::init);
        let docs = saule_docs::stdlib();
        let builtins = saule_semantic::builtins::snapshot();
        let mut missing = Vec::new();
        for class in ["Coroutine", "Yielder"] {
            let info = builtins
                .classes
                .get(class)
                .unwrap_or_else(|| panic!("`{class}` is not a builtin class"));
            assert!(!info.methods.is_empty(), "`{class}` declares no methods");
            for name in info.methods.keys() {
                let q = format!("{class}.{name}");
                if docs.entry(&q).is_none() {
                    missing.push(q);
                }
            }
        }
        assert!(
            missing.is_empty(),
            "builtin class methods with no entry in DOCS.md:\n  {}",
            missing.join("\n  ")
        );
    }

    #[test]
    fn names_agree_with_the_docs() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(saule_runtime::init);
        let docs = saule_docs::stdlib();
        let mut disagreements = Vec::new();
        for (qname, hand) in stdlib_table().iter() {
            let Some(entry) = docs.entry(qname) else {
                continue;
            };
            let Some(sg) = saule_typeck::sigs::lookup(qname) else {
                continue;
            };
            // Only the **positional** slots are comparable. The variadic one
            // is named by the table alone — the documentation declines to
            // name it, which is not a disagreement.
            let slots = sg.params.len();
            if slots == 0 || entry.params.is_empty() {
                continue;
            }
            let n = slots.min(hand.len()).min(entry.params.len());
            let a: Vec<&str> = hand.iter().take(n).copied().collect();
            let b: Vec<&str> = entry.params.iter().take(n).map(String::as_str).collect();
            if a != b {
                disagreements.push(format!(
                    "{qname}: table {a:?} vs DOCS.md {b:?} ({})",
                    entry.signature
                ));
            }
        }
        assert!(
            disagreements.is_empty(),
            "hand table contradicts the documentation:\n{}",
            disagreements.join("\n")
        );
    }

    /// Every documented function's names reach signature help.
    #[test]
    fn documented_names_are_used() {
        let s = sig(
            vec![
                Type::Named("string".into()),
                Type::Named("string".into()),
                Type::Nullable(Box::new(Type::Named("integer".into()))),
            ],
            None,
        );
        // Without the documentation these would be the type-derived
        // `s, s2, i` — `String.match` is not in the hand table.
        assert_eq!(param_names("String.match", &s), vec!["s", "pat", "from"]);
    }
}
