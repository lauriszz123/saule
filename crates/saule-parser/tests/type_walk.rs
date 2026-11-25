//! `saule_ast::visit_types_mut` reaches every place a type is written.
//!
//! `type` aliases are implemented as one rewrite over those places, so a
//! position this walk misses is a position where an alias silently stays a
//! name nobody knows. Each test here writes a marker type into one kind of
//! position and asserts the walk hands it over — with the type parameters
//! in scope there, which is what stops a parameter named like an alias
//! from being rewritten.

use saule_ast::{Module, Type, visit_types_mut};
use saule_lexer::Lexer;

fn parse(src: &str) -> Module {
    let tokens = Lexer::new(src).tokenize().expect("lex ok");
    saule_parser::parse(tokens).expect("parse ok")
}

/// Every name mentioned by a type, outermost first.
fn names(ty: &Type, out: &mut Vec<String>) {
    match ty {
        Type::Named(n) => out.push(n.clone()),
        Type::Nullable(inner) => names(inner, out),
        Type::Table { key, value } => {
            if let Some(k) = key {
                names(k, out);
            }
            names(value, out);
        }
        Type::Tuple(items) => items.iter().for_each(|t| names(t, out)),
        Type::Function { params, ret } => {
            params.iter().for_each(|t| names(t, out));
            names(ret, out);
        }
        Type::Generic(g) => {
            out.push(g.name.clone());
            g.args.iter().for_each(|t| names(t, out));
        }
    }
}

/// The marker names (`M1`, `M2`, …) the walk reaches in `src`, sorted.
fn reached(src: &str) -> Vec<String> {
    let mut module = parse(src);
    let mut seen = Vec::new();
    visit_types_mut(&mut module, |ty, _| names(ty, &mut seen));
    seen.retain(|n| n.starts_with('M') && n[1..].chars().all(|c| c.is_ascii_digit()));
    seen.sort();
    seen.dedup();
    seen
}

fn markers(n: usize) -> Vec<String> {
    let mut all: Vec<String> = (1..=n).map(|i| format!("M{i}")).collect();
    all.sort();
    all
}

#[test]
fn reaches_locals_loops_and_catches() {
    let src = "\
local a: M1 = nil
local b: M2, c: table<M3> = nil, nil
for i: M4 = 1, 2 do
end
for k: M5, v: M6 in {} do
end
try
catch e: M7
end
";
    assert_eq!(reached(src), markers(7));
}

#[test]
fn reaches_function_signatures_and_bodies() {
    let src = "\
fn f(a: M1, b: fn(M2) -> M3 = nil) -> M4
  local inner: M5 = nil
  local g = fn(x: M6) -> M7
    local deep: M8 = nil
    return nil
  end
  return nil
end
";
    assert_eq!(reached(src), markers(8));
}

#[test]
fn reaches_class_headers_fields_and_methods() {
    let src = "\
class C<T: Bound<M1>> extends Base<M2> implements Iface<M3>, Other<M4>
  field: M5 = nil
  static shared: table<string, M6> = {}
  fn method<U: Limit<M7>>(p: M8) -> M9
    local inside: M10 = nil
    return nil
  end
end
";
    assert_eq!(reached(src), markers(10));
}

#[test]
fn reaches_interfaces_and_enums() {
    let src = "\
interface I<T> extends Parent<M1>
  fn get(key: M2) -> M3
end
enum E<T>
  A(value: M4)
  B(x: M5, y: table<M6>)
  fn describe(with: M7) -> M8
    local l: M9 = nil
    return nil
  end
end
";
    assert_eq!(reached(src), markers(9));
}

#[test]
fn reaches_casts_type_arguments_and_module_variables() {
    let src = "\
export shared: M1 = nil
local a = nil as M2
local b = pick<M3, table<M4>>(1)
local c = when(items):filter<M5>(fn(x: M6) -> boolean
  return true
end)
type Alias<T> = table<T, M7>
";
    assert_eq!(reached(src), markers(7));
}

#[test]
fn reaches_types_inside_nested_blocks_and_expressions() {
    let src = "\
if true then
  while false do
    repeat
      local a: M1 = nil
    until true
  end
elseif false then
  local b: M2 = nil
else
  local c: M3 = nil
end
local t = { key: fn(x: M4) -> nil end, list: { nil as M5 } }
local m = match t
  case 1 then nil as M6
  case _ then
    local d: M7 = nil
    nil
end
";
    assert_eq!(reached(src), markers(7));
}

/// The scope handed over with each type: what a parameter's name means
/// there. Each position is annotated with a marker that says which.
#[test]
fn reports_the_type_parameters_in_scope() {
    let src = "\
local outside: P1 = nil
fn free<T>(a: P2) -> P3
  local in_body: P4 = nil
  return a
end
class Box<T>
  value: P5 = nil
  fn map<U>(f: fn(P6) -> U) -> P7
    local in_method: P8 = nil
    return f(self.value)
  end
  fn plain(x: P9) -> nil
  end
end
interface Source<T>
  fn next<U>(hint: P10) -> T
end
enum Option<T>
  Some(value: P11)
  None
end
type Callback<T> = fn(P12) -> nil
local after: P13 = nil
";
    let mut module = parse(src);
    let mut scopes: Vec<(String, Vec<String>)> = Vec::new();
    visit_types_mut(&mut module, |ty, site| {
        let mut mentioned = Vec::new();
        names(ty, &mut mentioned);
        for name in mentioned {
            scopes.push((name, site.type_params.to_vec()));
        }
    });
    let scope_at = |marker: &str| -> Vec<String> {
        scopes
            .iter()
            .find(|(name, _)| name == marker)
            .unwrap_or_else(|| panic!("no type visited at {marker}: {scopes:?}"))
            .1
            .clone()
    };
    let none: [&str; 0] = [];

    assert_eq!(scope_at("P1"), none, "module level has none");
    assert_eq!(scope_at("P2"), ["T"]);
    assert_eq!(scope_at("P3"), ["T"]);
    assert_eq!(
        scope_at("P4"),
        ["T"],
        "a body is inside its function's scope"
    );
    assert_eq!(scope_at("P5"), ["T"]);
    assert_eq!(scope_at("P6"), ["T", "U"]);
    assert_eq!(scope_at("P7"), ["T", "U"]);
    assert_eq!(scope_at("P8"), ["T", "U"]);
    assert_eq!(scope_at("P9"), ["T"], "a sibling method's `U` is gone");
    assert_eq!(scope_at("P10"), ["T", "U"]);
    assert_eq!(scope_at("P11"), ["T"]);
    assert_eq!(scope_at("P12"), ["T"], "an alias's own parameter");
    assert_eq!(scope_at("P13"), none, "nothing leaks out of a declaration");
}

/// Where each type is said to be: the annotation itself when the tree
/// records one, otherwise the smallest thing around it.
#[test]
fn reports_where_each_type_stands() {
    let src = "\
local a: table<M1> = nil
fn f(p: M2) -> M3
  return nil as M4
end
type Alias = fn(M5) -> nil
";
    let mut module = parse(src);
    let mut at: Vec<(String, String)> = Vec::new();
    visit_types_mut(&mut module, |ty, site| {
        let mut mentioned = Vec::new();
        names(ty, &mut mentioned);
        for name in mentioned {
            at.push((name, src[site.span.clone()].to_string()));
        }
    });
    let text = |marker: &str| -> &str {
        &at.iter()
            .find(|(name, _)| name == marker)
            .unwrap_or_else(|| panic!("no type visited at {marker}"))
            .1
    };
    assert_eq!(text("M1"), "table<M1>", "a local's annotation");
    assert_eq!(text("M2"), "p: M2", "a parameter");
    assert!(
        text("M3").starts_with("fn f(p: M2) -> M3"),
        "a result: the declaration"
    );
    assert_eq!(text("M4"), "nil as M4", "a cast");
    assert_eq!(text("M5"), "fn(M5) -> nil", "an alias's right-hand side");
}

/// The callback gets each type whole and may replace it.
#[test]
fn rewrites_in_place() {
    let mut module = parse("local a: Old = nil\nfn f(x: table<Old>) -> Old?\n  return nil\nend\n");
    fn rename(ty: &mut Type) {
        match ty {
            Type::Named(n) if n == "Old" => *n = "New".into(),
            Type::Named(_) => {}
            Type::Nullable(inner) => rename(inner),
            Type::Table { key, value } => {
                if let Some(k) = key {
                    rename(k);
                }
                rename(value);
            }
            Type::Tuple(items) => items.iter_mut().for_each(rename),
            Type::Function { params, ret } => {
                params.iter_mut().for_each(rename);
                rename(ret);
            }
            Type::Generic(g) => g.args.iter_mut().for_each(rename),
        }
    }
    visit_types_mut(&mut module, |ty, _| rename(ty));

    let mut seen = Vec::new();
    visit_types_mut(&mut module, |ty, _| names(ty, &mut seen));
    assert!(seen.contains(&"New".to_string()));
    assert!(!seen.contains(&"Old".to_string()), "{seen:?}");
}

/// Numbering shares this traversal, and must not be disturbed by it.
#[test]
fn leaves_node_ids_alone() {
    let src = "class C<T>\n  v: T = nil\n  fn m(x: T) -> T\n    return x as T\n  end\nend\n";
    let mut module = parse(src);
    let before = format!("{:?}", collect_ids(&module));
    visit_types_mut(&mut module, |_, _| {});
    assert_eq!(before, format!("{:?}", collect_ids(&module)));
}

fn collect_ids(module: &Module) -> Vec<saule_ast::NodeId> {
    let mut ids = Vec::new();
    saule_ast::visit_exprs(module, &mut |e| ids.push(e.id));
    ids
}
