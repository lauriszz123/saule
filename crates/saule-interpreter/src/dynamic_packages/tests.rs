use super::*;
use crate::value::{TableObject, Value};
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
