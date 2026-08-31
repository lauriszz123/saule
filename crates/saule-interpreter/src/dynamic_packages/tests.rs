use super::*;
use crate::error::RuntimeError;
use crate::value::{TableObject, Value};
use saule_ast::Type;
use std::cell::RefCell;
use std::rc::Rc;

#[test]
fn parses_a_manifest() {
    let text = r#"
            [package]
            name = "engine"
            version = "0.1.0"
            binary = "engine.so, engine.dll, engine.dylib"

            [exports.Graphics]
            type = "class"
            doc = "2D graphics"

              [[exports.Graphics.methods]]
              name = "circle"
              sig = "fn(mode: string, x: float, y: float, radius: float) -> nil"
              native_symbol = "saule_engine_graphics_circle"
        "#;
    let m = parse_manifest(text).expect("manifest should parse");
    assert_eq!(m.name, "engine");
    assert_eq!(m.binaries, ["engine.so", "engine.dll", "engine.dylib"]);
    assert_eq!(m.exports.len(), 1);
    let g = &m.exports[0];
    assert_eq!(g.name, "Graphics");
    assert_eq!(g.methods.len(), 1);
    assert_eq!(g.methods[0].name, "circle");
    assert_eq!(g.methods[0].symbol, "saule_engine_graphics_circle");
    assert_eq!(g.methods[0].param_names, ["mode", "x", "y", "radius"]);
    assert_eq!(g.methods[0].params.len(), 4);
    assert_eq!(g.methods[0].returns.len(), 1);
}

#[test]
fn splits_nested_commas() {
    let parts = split_top_level("a: table<K, V>, b: float");
    assert_eq!(parts, ["a: table<K, V>", "b: float"]);
}

#[test]
fn parses_tuple_return() {
    let (_g, _n, _p, r) = parse_sig("fn() -> (integer, integer)").unwrap();
    assert_eq!(r.len(), 2);
}

#[test]
fn spreads_table_into_multiple_returns() {
    let table = Value::Table(Rc::new(RefCell::new(TableObject::from_array(vec![
        Value::Int(3),
        Value::Int(2),
    ]))));
    let out = spread_multi_return(table, 2);
    assert!(matches!(out.as_slice(), [Value::Int(3), Value::Int(2)]));
}

#[test]
fn spreads_pads_missing_slots_with_nil() {
    // A short table (or a misbehaving native) still yields `arity` values.
    let table = Value::Table(Rc::new(RefCell::new(TableObject::from_array(vec![
        Value::Int(1),
    ]))));
    let out = spread_multi_return(table, 3);
    assert!(matches!(
        out.as_slice(),
        [Value::Int(1), Value::Nil, Value::Nil]
    ));
}

#[test]
fn spreads_non_table_result_as_first_value() {
    let out = spread_multi_return(Value::Int(7), 2);
    assert!(matches!(out.as_slice(), [Value::Int(7), Value::Nil]));
}

#[test]
fn parses_generic_prefix() {
    let (generics, names, params, returns) =
        parse_sig("fn<T>(t: table<T>, value: T) -> T?").unwrap();
    assert_eq!(generics, ["T"]);
    assert_eq!(names, ["t", "value"]);
    assert_eq!(params.len(), 2);
    assert_eq!(returns.len(), 1);
}

/// A callback parameter's signature has to survive the manifest round-trip.
/// It used to fall through to `Type::Named("fn(T) -> boolean")` — a name no
/// substitution could reach — so `T` stayed unbound and every lambda passed
/// to such a parameter was reported as the wrong type.
#[test]
fn parses_a_function_typed_parameter() {
    let (generics, names, params, _r) =
        parse_sig("fn<T>(t: table<T>, f: fn(T) -> boolean) -> table<T>").unwrap();
    assert_eq!(generics, ["T"]);
    assert_eq!(names, ["t", "f"]);
    assert_eq!(
        params[1],
        Type::Function {
            params: vec![Type::Named("T".into())],
            ret: Box::new(Type::Named("boolean".into())),
        }
    );
}

/// The `>` in `->` is not a closing bracket. While it was counted as one the
/// depth went negative and the comma before `init` looked nested, so this
/// signature parsed as two parameters instead of three.
#[test]
fn a_comma_in_a_callback_does_not_swallow_the_next_parameter() {
    let (generics, names, params, _r) =
        parse_sig("fn<T, U>(t: table<T>, f: fn(U, T) -> U, init: U) -> U").unwrap();
    assert_eq!(generics, ["T", "U"]);
    assert_eq!(names, ["t", "f", "init"]);
    assert_eq!(
        params[1],
        Type::Function {
            params: vec![Type::Named("U".into()), Type::Named("T".into())],
            ret: Box::new(Type::Named("U".into())),
        }
    );
    assert_eq!(params[2], Type::Named("U".into()));
}

/// `function` is not a type — a callback names the calls it accepts. A
/// manifest generated before that rule still spells one out, and parsing it
/// into a named type would hand the checker something no lambda unifies with,
/// so every call into the package would be reported as an argument-type error
/// with nothing in the user's own source to fix. The manifest is rejected at
/// load instead, in every position it can appear in.
#[test]
fn a_bare_function_type_is_rejected() {
    for sig in [
        "fn<T>(t: table<T>, f: function) -> table<T>",
        "fn(f: function?) -> nil",
        "fn(fs: table<function>) -> nil",
        "fn(f: fn(function) -> nil) -> nil",
        "fn() -> function",
        "fn() -> (integer, function)",
    ] {
        let err = parse_sig(sig).expect_err(&format!("`{sig}` must not parse"));
        assert!(err.contains("`function` is not a type"), "got: {err}");
    }
}

/// `?` binds to the return type inside a function type, so a *nullable
/// callback* has to be parenthesised — and the parenthesised form has to
/// come back as a nullable function, not as its own return type.
#[test]
fn parses_nullable_and_returning_nullable_callbacks() {
    let (_g, _n, params, _r) =
        parse_sig("fn(a: (fn(string) -> nil)?, b: fn() -> integer?) -> nil").unwrap();
    assert_eq!(
        params[0],
        Type::Nullable(Box::new(Type::Function {
            params: vec![Type::Named("string".into())],
            ret: Box::new(Type::Named("nil".into())),
        }))
    );
    assert_eq!(
        params[1],
        Type::Function {
            params: vec![],
            ret: Box::new(Type::Nullable(Box::new(Type::Named("integer".into())))),
        }
    );
}

#[test]
fn parses_param_names_with_fallback_for_unnamed() {
    let (_g, names, params, _r) = parse_sig("fn(integer, y: float) -> nil").unwrap();
    assert_eq!(names, ["arg0", "y"]);
    assert_eq!(params.len(), 2);
}

#[test]
fn class_info_uses_manifest_param_names() {
    let text = r#"
            [package]
            name = "engine"
            version = "0.1.0"
            binary = "engine.so"

            [exports.Graphics]
              [[exports.Graphics.methods]]
              name = "circle"
              sig = "fn(mode: string, x: float, y: float, radius: float) -> nil"
              native_symbol = "saule_engine_graphics_circle"
        "#;
    let m = parse_manifest(text).expect("manifest should parse");
    let info = class_info(&m.exports[0]);
    let sig = info.methods.get("circle").expect("circle method");
    let names: Vec<&str> = sig.params.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["mode", "x", "y", "radius"]);
}

/// Building a package's surface for the *compiler* must not touch the
/// binary. The proof is that this manifest names one that does not exist and
/// building still succeeds — if a `dlopen` had happened it could not have.
///
/// This is what lets a dynamic package be folded into constants at compile
/// time, which is the whole reason `saule-vm` can compile an import of one
/// (`VM_TASKS.md`, "an import of a dynamic native package").
#[cfg(feature = "native-packages")]
#[test]
fn a_deferred_binding_loads_nothing_until_it_is_called() {
    let text = r#"
            [package]
            name = "nosuchpkg"
            version = "0.1.0"
            binary = "nosuchpkg-not-installed.so"

            [exports.Graphics]
              [[exports.Graphics.methods]]
              name = "circle"
              sig = "fn(mode: string, x: float, y: float, radius: float) -> nil"
              native_symbol = "nosuchpkg_graphics_circle"
        "#;
    let m = parse_manifest(text).expect("manifest should parse");
    let class = Rc::new(build_class_deferred(&m.exports[0], &m.name));

    let Some(Value::NativeClosure(f)) = class.lookup_static_field("circle") else {
        panic!("`circle` should bind to a native closure");
    };
    // The manifest's parameter names ride along, so named arguments work
    // exactly as they do on the eager path.
    assert_eq!(f.param_names, ["mode", "x", "y", "radius"]);

    // Calling is where the load is attempted — and where it fails, because
    // nothing ever registered this package. A deferred binding that could
    // not resolve reports; it does not dangle.
    let err = (f.func)(&[]).expect_err("an unregistered package cannot resolve");
    assert!(err.contains("nosuchpkg"), "got: {err}");
}

/// `preload` is the side-effecting half `saule-vm` calls at run time, in
/// place of the import-time load the tree-walker does. Its failure has to be
/// the same shape — an `ImportError` carrying the `import`'s own span — or
/// the two engines would report a broken package differently.
#[test]
fn preload_reports_an_unregistered_package_at_the_import_span() {
    let err = preload("nosuchpkg", 3..9).expect_err("an unregistered package cannot load");
    match err {
        RuntimeError::ImportError { message, span } => {
            assert!(message.contains("nosuchpkg"), "got: {message}");
            assert_eq!(span, 3..9);
        }
        other => panic!("expected an ImportError, got: {other:?}"),
    }
}
