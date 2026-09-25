use super::embedded::RawRecord;
use super::*;
use crate::error::RuntimeError;
use crate::value::{TableObject, Value};
use saule_ast::Type;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

// ─── Metadata records → a package ───────────────────────────────────────────
//
// The records here are spelled the way `saule-export-macro` writes them, so
// these tests hold the two ends of the format to the same contract.

fn rec(symbol: &str, payload: &str) -> RawRecord {
    RawRecord {
        symbol: symbol.to_string(),
        payload: payload.to_string(),
    }
}

fn package_rec() -> RawRecord {
    rec(
        "PACKAGE",
        &format!(
            "kind = \"package\"\nname = \"gfx\"\nversion = \"0.1.0\"\nabi_version = {}\n",
            saule_native_abi::ABI_VERSION
        ),
    )
}

fn method_rec(class: &str, name: &str, receiver: &str, sig: &str) -> RawRecord {
    rec(
        &format!("M_{class}_{name}"),
        &format!(
            "kind = \"method\"\nclass = \"{class}\"\nname = \"{name}\"\nreceiver = \
             \"{receiver}\"\nsig = \"{sig}\"\nsymbol = \"saule_export_{class}_{name}\"\n"
        ),
    )
}

fn class_rec(name: &str, instantiable: bool) -> RawRecord {
    rec(
        &format!("C_{name}"),
        &format!(
            "kind = \"class\"\nname = \"{name}\"\ndoc = \"The {name} class.\"\n\
             instantiable = {instantiable}\n"
        ),
    )
}

fn assemble(records: &[RawRecord]) -> Result<Manifest, String> {
    manifest_from_records(records, Path::new("/pkgs/libgfx.so"))
}

#[test]
fn assembles_a_package_from_its_records() {
    let m = assemble(&[
        package_rec(),
        class_rec("Image", true),
        method_rec(
            "Image",
            "init",
            "constructor",
            "fn(w: integer, h: integer) -> Image",
        ),
        method_rec("Image", "width", "getter", "fn() -> integer"),
        method_rec("Image", "fill", "instance", "fn(color: integer) -> nil"),
        method_rec("Image", "load", "static", "fn(path: string) -> Image"),
        // A namespace nobody declared: synthesised, with no doc.
        method_rec(
            "Graphics",
            "circle",
            "static",
            "fn(mode: string, x: float, y: float, radius: float) -> nil",
        ),
        rec(
            "E_Blend",
            "kind = \"enum\"\nname = \"Blend\"\nvariants = [\"Alpha\", \"Add\"]\n\
             variant_docs = [\"\", \"Additive.\"]\n",
        ),
    ])
    .expect("the package should assemble");

    assert_eq!(m.name, "gfx");
    assert_eq!(m.path, Path::new("/pkgs/libgfx.so"));
    // Sorted by name, so everything built from it is deterministic.
    let names: Vec<&str> = m.exports.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Graphics", "Image"]);

    let graphics = &m.exports[0];
    assert!(!graphics.instantiable);
    assert_eq!(graphics.doc, None);
    assert_eq!(
        graphics.methods[0].param_names,
        ["mode", "x", "y", "radius"]
    );

    let image = &m.exports[1];
    assert!(image.instantiable);
    assert_eq!(image.doc.as_deref(), Some("The Image class."));
    assert_eq!(image.constructor().map(|c| c.params.len()), Some(2));
    assert_eq!(image.members(Receiver::Instance).count(), 1);
    assert_eq!(image.members(Receiver::Getter).count(), 1);
    assert_eq!(image.members(Receiver::Static).count(), 1);

    assert_eq!(m.enums[0].variants, ["Alpha", "Add"]);
}

#[test]
fn a_package_with_no_package_record_is_refused() {
    let err = assemble(&[method_rec("Util", "f", "static", "fn() -> nil")])
        .expect_err("records without a package are not a package");
    assert!(err.contains("saule_package!"), "{err}");
}

#[test]
fn a_package_built_for_another_abi_is_refused() {
    let theirs = saule_native_abi::ABI_VERSION + 1;
    let record = rec(
        "PACKAGE",
        &format!(
            "kind = \"package\"\nname = \"gfx\"\nversion = \"0.1.0\"\nabi_version = {theirs}\n"
        ),
    );
    let err = assemble(&[record]).expect_err("a mismatched ABI must be refused");
    // Both numbers, so the reader can tell which side is stale.
    assert!(err.contains(&theirs.to_string()), "{err}");
    assert!(
        err.contains(&saule_native_abi::ABI_VERSION.to_string()),
        "{err}"
    );
}

#[test]
fn a_package_that_declares_no_abi_is_refused() {
    let record = rec(
        "PACKAGE",
        "kind = \"package\"\nname = \"gfx\"\nversion = \"0.1.0\"\n",
    );
    let err = assemble(&[record]).expect_err("an unversioned package must be refused");
    assert!(err.contains("ABI version"), "{err}");
}

/// An object's member on a class that has no objects is a contradiction the
/// checker would otherwise take at face value.
#[test]
fn an_instance_member_needs_an_instantiable_class() {
    let err = assemble(&[
        package_rec(),
        class_rec("Util", false),
        method_rec("Util", "size", "instance", "fn() -> integer"),
    ])
    .expect_err("an instance method on a namespace is refused");
    assert!(err.contains("Util.size"), "{err}");
}

#[test]
fn a_class_has_one_constructor() {
    let mut second = method_rec("Image", "init", "constructor", "fn(path: string) -> Image");
    second.symbol = "N_Image_2".to_string();
    let err = assemble(&[
        package_rec(),
        class_rec("Image", true),
        method_rec("Image", "init", "constructor", "fn() -> Image"),
        second,
    ])
    .expect_err("two constructors are refused");
    assert!(err.contains("constructor"), "{err}");
}

#[test]
fn a_member_cannot_be_both_a_method_and_a_property() {
    let err = assemble(&[
        package_rec(),
        class_rec("Image", true),
        method_rec("Image", "width", "getter", "fn() -> integer"),
        method_rec("Image", "width", "instance", "fn() -> integer"),
    ])
    .expect_err("a name is either a method or a property");
    assert!(err.contains("Image.width"), "{err}");
}

/// A getter and a setter of one name are one property, not a clash.
#[test]
fn a_getter_and_setter_share_a_name() {
    let m = assemble(&[
        package_rec(),
        class_rec("Image", true),
        method_rec("Image", "width", "getter", "fn() -> integer"),
        method_rec("Image", "width", "setter", "fn(value: integer) -> nil"),
    ])
    .expect("a readable, writable property");
    assert_eq!(m.exports[0].methods.len(), 2);
}

/// A kind a newer SDK writes is skipped, so an older toolchain can still use
/// the parts of a newer package it understands.
#[test]
fn an_unknown_record_kind_is_ignored() {
    let m = assemble(&[
        package_rec(),
        rec("X_future", "kind = \"interface\"\nname = \"Drawable\"\n"),
    ])
    .expect("an unknown kind is not an error");
    assert!(m.exports.is_empty());
}

#[test]
fn a_record_that_is_not_toml_names_itself() {
    let err = assemble(&[package_rec(), rec("M_Util_f", "kind = = broken")])
        .expect_err("garbage is refused");
    assert!(err.contains("M_Util_f"), "{err}");
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
fn class_info_uses_the_signatures_param_names() {
    let m = assemble(&[
        package_rec(),
        method_rec(
            "Graphics",
            "circle",
            "static",
            "fn(mode: string, x: float, y: float, radius: float) -> nil",
        ),
    ])
    .expect("the package should assemble");
    let info = class_info(&m.exports[0]);
    let sig = info.methods.get("circle").expect("circle method");
    let names: Vec<&str> = sig.params.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["mode", "x", "y", "radius"]);
}

/// What the checker is told about a class with objects: a constructor named
/// `init` (as a Saule class's is), instance methods that are not static, and
/// properties as typed fields.
#[test]
fn class_info_describes_objects_like_a_saule_class() {
    let m = assemble(&[
        package_rec(),
        class_rec("Image", true),
        method_rec("Image", "init", "constructor", "fn(w: integer) -> Image"),
        method_rec("Image", "fill", "instance", "fn(color: integer) -> nil"),
        method_rec("Image", "load", "static", "fn(path: string) -> Image"),
        method_rec("Image", "width", "getter", "fn() -> integer"),
    ])
    .expect("the package should assemble");
    let info = class_info(&m.exports[0]);

    let init = info.methods.get("init").expect("a constructor");
    assert!(!init.is_static);
    assert_eq!(init.return_ty, None, "`init` returns nothing, as in Saule");
    assert!(!info.methods["fill"].is_static);
    assert!(info.methods["load"].is_static);
    assert_eq!(info.field_types["width"], Type::Named("integer".into()));
    assert!(
        !info.methods.contains_key("width"),
        "a property is not a method"
    );
}

/// `preload` is the side-effecting half `saule-vm` calls at run time. Its
/// failure has to be an `ImportError` carrying the `import`'s own span, so a
/// broken package is reported at the line that asked for it.
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
