use super::*;
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
/// to `Util.filter` was reported as the wrong type.
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

/// `?` binds to the return type inside a function type, so a *nullable
/// callback* has to be parenthesised — and the parenthesised form has to
/// come back as a nullable function, not as its own return type.
#[test]
fn parses_nullable_and_returning_nullable_callbacks() {
    let (_g, _n, params, _r) = parse_sig("fn(a: (fn(string) -> nil)?, b: fn() -> integer?) -> nil")
        .unwrap();
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
