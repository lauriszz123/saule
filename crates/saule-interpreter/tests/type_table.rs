//! Phase 0.5: the type table `saule-typeck` now publishes.
//!
//! Lives here rather than in `saule-typeck` because typecheck has a
//! precondition — the semantic registries and the stdlib's native signatures
//! must be installed first — and this crate is where both come from.
//!
//! Two properties matter, and they pull in opposite directions:
//!
//! * the table says the right thing where it says anything, because the
//!   compiler will select opcodes from it;
//! * asking for it does not change a single diagnostic, because `check` is
//!   the language's contract and the VM is an implementation change.

use saule_ast::{Module, NodeId, Type};
use saule_interpreter::typeck::{self, TypeTable};
use saule_lexer::Lexer;
use saule_parser::parse;

fn front_end(src: &str) -> Module {
    saule_interpreter::init();
    let toks = Lexer::new(src).tokenize().expect("lex");
    let module = parse(toks).expect("parse");
    let seed = saule_semantic::ModuleSeed::default();
    let errs = saule_interpreter::semantic::analyze_with_seed(&module, seed);
    assert!(errs.is_empty(), "semantic errors: {errs:?}");
    module
}

fn table_of(src: &str) -> (Module, TypeTable) {
    let module = front_end(src);
    let (errors, table) = typeck::check_with_types(&module);
    assert!(errors.is_empty(), "typeck errors: {errors:?}");
    (module, table)
}

/// The recorded type of the first expression matching `pred`, in pre-order.
fn first_typed<F: Fn(&saule_ast::Expr) -> bool>(
    module: &Module,
    table: &TypeTable,
    pred: F,
) -> Option<Type> {
    let mut found = None;
    saule_ast::visit_exprs(module, &mut |e| {
        if found.is_none() && pred(&e.value) {
            found = Some(table.get(&e.id).cloned());
        }
    });
    found.flatten()
}

fn named(t: &Type) -> &str {
    match t {
        Type::Named(n) => n.as_str(),
        other => panic!("expected a named type, got {other:?}"),
    }
}

#[test]
fn records_literal_operand_types() {
    let (m, table) = table_of("local x: integer = 1 + 2");
    // Both operands *and* the binary itself — the three nodes the compiler
    // consults to choose `ADDI` over `ARITHX`.
    let lhs = first_typed(&m, &table, |e| matches!(e, saule_ast::Expr::Int(1))).expect("lhs typed");
    let rhs = first_typed(&m, &table, |e| matches!(e, saule_ast::Expr::Int(2))).expect("rhs typed");
    let whole = first_typed(&m, &table, |e| matches!(e, saule_ast::Expr::Binary { .. }))
        .expect("binary typed");
    assert_eq!(named(&lhs), "integer");
    assert_eq!(named(&rhs), "integer");
    assert_eq!(named(&whole), "integer");
}

#[test]
fn distinguishes_integer_from_float() {
    let (m, table) = table_of("local x: float = 1.5 * 2.0");
    let whole = first_typed(&m, &table, |e| matches!(e, saule_ast::Expr::Binary { .. }))
        .expect("binary typed");
    assert_eq!(named(&whole), "float");
}

#[test]
fn records_the_declared_type_of_a_local_read() {
    let (m, table) = table_of(
        r#"
local n: integer = 3
local m: integer = n + 1
"#,
    );
    let ident = first_typed(
        &m,
        &table,
        |e| matches!(e, saule_ast::Expr::Ident(s) if s == "n"),
    )
    .expect("identifier typed");
    assert_eq!(named(&ident), "integer");
}

#[test]
fn records_types_inside_a_function_body() {
    // Bodies are walked with their own scope; a table that only covered the
    // top level would be useless to a compiler.
    let (m, table) = table_of(
        r#"
fn add(a: integer, b: integer) -> integer
  return a + b
end
"#,
    );
    let ident = first_typed(
        &m,
        &table,
        |e| matches!(e, saule_ast::Expr::Ident(s) if s == "a"),
    )
    .expect("parameter read typed");
    assert_eq!(named(&ident), "integer");
}

#[test]
fn asking_for_types_does_not_change_diagnostics() {
    // The property that keeps `check` the contract: same walk, plus a sink.
    for src in [
        "local x: integer = 1 + 2",
        "local x: integer = \"nope\"",
        "local s: string? = nil\nlocal n = s.len",
        "fn f() -> integer\n  return\nend",
        "local t = {1, 2, 3}\nlocal v: string = t[1]",
    ] {
        saule_interpreter::init();
        let toks = Lexer::new(src).tokenize().expect("lex");
        let module = parse(toks).expect("parse");
        let seed = saule_semantic::ModuleSeed::default();
        let _ = saule_interpreter::semantic::analyze_with_seed(&module, seed);

        let plain: Vec<String> = typeck::check(&module).iter().map(|e| e.to_string()).collect();
        let (with_types, _) = typeck::check_with_types(&module);
        let collected: Vec<String> = with_types.iter().map(|e| e.to_string()).collect();

        assert_eq!(plain, collected, "diagnostics diverged for:\n{src}");
    }
}

#[test]
fn node_id_none_is_never_a_key() {
    // The documented precondition, from the other side. A hand-built tree —
    // the shape parser tests use — has every node at `NodeId::NONE`. Those
    // must be *dropped*, not all written to one shared key, which would turn
    // "no ids" into confident nonsense the compiler would act on.
    let (_, table) = table_of("local x: integer = 1 + 2\nlocal y: float = 1.5 - 0.5");
    assert!(!table.is_empty(), "a numbered tree should populate the table");
    assert!(!table.contains_key(&NodeId::NONE), "NONE must never be a key");
}

#[test]
fn arithmetic_operand_coverage_stays_above_the_bar() {
    // §24.1 names this as the first risk to the whole VM project: if
    // inference misses arithmetic operands, everything degrades to the
    // dynamic `ARITHX` form and the projected speed-up collapses. Measured
    // at 100% across `benchmarks/sau` when this landed; the assertion is set
    // well below that so it catches a real regression, not noise.
    let src = r#"
fn fib(n: integer) -> integer
  if n < 2 then return n end
  return fib(n - 1) + fib(n - 2)
end

fn mix(a: float, b: float) -> float
  return a * b + a / b - b % a
end

fn bits(x: integer) -> integer
  return (x << 2) | (x >> 1) & 255
end

local total: integer = 0
for i = 1, 10 do
  total = total + i * 2
end
"#;
    let (m, table) = table_of(src);
    let c = typeck::coverage::measure(&m, &table);
    assert!(c.arith_operands > 20, "test source shrank: {}", c.summary());
    assert!(
        c.arith_numeric_percent() >= 90.0,
        "arithmetic operand coverage fell below the §24.1 bar: {}",
        c.summary()
    );
}
