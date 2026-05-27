//! Tree-walking interpreter for the Saule language.
//!
//! Module layout:
//!
//! | Module       | Responsibility                              |
//! |--------------|---------------------------------------------|
//! | [`value`]    | Runtime [`Value`] enum and `NativeFn`       |
//! | [`env`]      | Lexical scopes ([`Environment`])            |
//! | [`stdlib`]   | Standard library installed into the prelude |
//! | [`error`]    | [`RuntimeError`] (miette-aware diagnostics) |
//! | [`eval`]     | Statement & expression evaluation           |
//!
//! Phase status:
//!   * Phase 1 — literals, locals, arithmetic, native calls (✓)
//!   * Phase 2 — assignment, blocks, `if`/`while`/`repeat`/numeric `for`,
//!     `break`/`continue`, lexical scoping (✓)
//!   * Phase 3 — user-defined functions, lambdas, closures, `return` (✓ —
//!     this commit)
//!   * Phase 4 — tables and indexing (next)

use std::cell::RefCell;
use std::rc::Rc;

use saule_ast::Module;

pub mod env;
pub mod error;
pub mod eval;
pub mod stdlib;
pub mod typeck;
pub mod value;
pub mod module;

pub use env::Environment;
pub use error::RuntimeError;
pub use eval::Flow;
pub use value::{NativeFn, Value};

/// Invoke a [`value::FunctionObject`] with the given arguments. Exposed so
/// embedders (the CLI, the REPL, future test runners) can call functions
/// that were defined in user code without re-parsing.
pub fn call_function_value(
    f: &std::rc::Rc<value::FunctionObject>,
    args: &[Value],
) -> Result<Value, RuntimeError> {
    let evaled: Vec<eval::expr::EvaluatedArg> = args
        .iter()
        .cloned()
        .map(eval::expr::EvaluatedArg::Positional)
        .collect();
    eval::expr::call_function(f, &evaled, 0..0)
}

/// Invoke a static method on a class, with `self` bound to the class — the
/// CLI uses this to run `Main.main()`.
pub fn call_class_static_method(
    class: &std::rc::Rc<value::ClassObject>,
    method: &str,
    args: &[Value],
) -> Result<Value, RuntimeError> {
    let f = class
        .lookup_static_method(method)
        .ok_or_else(|| RuntimeError::TypeError {
            message: format!("no static method `{method}` on class `{}`", class.name),
            span: 0..0,
        })?;
    let evaled: Vec<eval::expr::EvaluatedArg> = args
        .iter()
        .cloned()
        .map(eval::expr::EvaluatedArg::Positional)
        .collect();
    eval::expr::call_static_method_public(&f, class, &evaled, 0..0)
}

/// Run a parsed [`Module`] in a fresh environment seeded with built-ins.
///
/// Returns the value of the last evaluated expression-statement (useful for
/// the REPL and for tests).
pub fn run(module: &Module) -> Result<Value, RuntimeError> {
    let env = Environment::with_prelude();
    run_in(module, &env)
}

/// Run a [`Module`] inside a caller-supplied environment.
pub fn run_in(module: &Module, env: &Rc<RefCell<Environment>>) -> Result<Value, RuntimeError> {
    match eval::stmt::exec_block(&module.stmts, env)? {
        Flow::Normal(v) => Ok(v),
        // At the top level these are illegal — `Stmt::Return` is rejected
        // inside `exec`; `break`/`continue` reach here only if they appear
        // outside any loop.
        Flow::Break => Err(RuntimeError::LoopControlOutsideLoop {
            which: "break",
            span: 0..0,
        }),
        Flow::Continue => Err(RuntimeError::LoopControlOutsideLoop {
            which: "continue",
            span: 0..0,
        }),
        Flow::Return(values) => Ok(values.into_iter().next().unwrap_or(Value::Nil)),
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use saule_lexer::Lexer;
    use saule_parser::parse;
    use std::rc::Rc;

    fn eval(src: &str) -> Result<Value, RuntimeError> {
        let toks = Lexer::new(src).tokenize().expect("lex");
        let module = parse(toks).expect("parse");
        run(&module)
    }

    // ── Phase 1 regression coverage ──────────────────────────────────────────

    #[test]
    fn integer_arithmetic() {
        assert_eq!(eval("1 + 2 * 3").unwrap(), Value::Int(7));
    }

    #[test]
    fn float_arithmetic() {
        match eval("1.5 + 2.5").unwrap() {
            Value::Float(f) => assert_eq!(f, 4.0),
            v => panic!("expected float, got {v:?}"),
        }
    }

    #[test]
    fn mixing_int_and_float_errors() {
        assert!(matches!(
            eval("1 + 2.0").unwrap_err(),
            RuntimeError::NumericMix { .. }
        ));
    }

    #[test]
    fn local_then_lookup() {
        assert_eq!(
            eval("local x: integer = 10\nx * 2").unwrap(),
            Value::Int(20)
        );
    }

    #[test]
    fn string_concat_and_length() {
        assert_eq!(
            eval(r#""foo" .. "bar""#).unwrap(),
            Value::Str(Rc::new("foobar".into()))
        );
        assert_eq!(eval(r#"#"hello""#).unwrap(), Value::Int(5));
    }

    #[test]
    fn comparison_and_logic() {
        assert_eq!(eval("1 < 2 and 3 >= 3").unwrap(), Value::Bool(true));
        assert_eq!(
            eval(r#"nil or "fallback""#).unwrap(),
            Value::Str(Rc::new("fallback".into()))
        );
    }

    #[test]
    fn null_coalescing() {
        assert_eq!(
            eval(r#"nil ?? "x""#).unwrap(),
            Value::Str(Rc::new("x".into()))
        );
        assert_eq!(
            eval(r#""y" ?? "x""#).unwrap(),
            Value::Str(Rc::new("y".into()))
        );
    }

    #[test]
    fn print_native_call() {
        assert_eq!(eval(r#"print("hello", 1, true)"#).unwrap(), Value::Nil);
    }

    #[test]
    fn undefined_variable_errors() {
        assert!(matches!(
            eval("nope").unwrap_err(),
            RuntimeError::Undefined { .. }
        ));
    }

    #[test]
    fn division_by_zero() {
        assert!(matches!(
            eval("1 / 0").unwrap_err(),
            RuntimeError::DivisionByZero { .. }
        ));
    }

    #[test]
    fn type_builtin() {
        assert_eq!(
            eval("type(42)").unwrap(),
            Value::Str(Rc::new("integer".into()))
        );
        assert_eq!(
            eval(r#"type("hi")"#).unwrap(),
            Value::Str(Rc::new("string".into()))
        );
    }

    // ── Phase 2: assignment ──────────────────────────────────────────────────

    #[test]
    fn assignment_updates_local() {
        let src = "local x: integer = 1\nx = x + 41\nx";
        assert_eq!(eval(src).unwrap(), Value::Int(42));
    }

    #[test]
    fn assign_to_undeclared_errors() {
        let src = "x = 1";
        assert!(matches!(
            eval(src).unwrap_err(),
            RuntimeError::AssignUndeclared { .. }
        ));
    }

    // ── Phase 2: if / elseif / else ──────────────────────────────────────────

    #[test]
    fn if_then_branch() {
        let src = r#"
            local x: integer = 0
            if true then
              x = 1
            else
              x = 2
            end
            x
        "#;
        assert_eq!(eval(src).unwrap(), Value::Int(1));
    }

    #[test]
    fn if_elseif_else_chain() {
        let src = r#"
            local n: integer = 7
            local kind: string = ""
            if n < 0 then
              kind = "neg"
            else if n == 0 then
              kind = "zero"
            else if n < 10 then
              kind = "small"
            else
              kind = "big"
            end
            kind
        "#;
        assert_eq!(eval(src).unwrap(), Value::Str(Rc::new("small".into())));
    }

    #[test]
    fn if_body_introduces_scope() {
        // `tmp` is declared inside the `if` body and must NOT leak out.
        let src = r#"
            local x: integer = 0
            if true then
              local tmp: integer = 9
              x = tmp
            end
            tmp
        "#;
        assert!(matches!(
            eval(src).unwrap_err(),
            RuntimeError::Undefined { .. }
        ));
    }

    // ── Phase 2: while + break + continue ────────────────────────────────────

    #[test]
    fn while_counts_to_ten() {
        let src = r#"
            local i: integer = 0
            local sum: integer = 0
            while i < 10 do
              i = i + 1
              sum = sum + i
            end
            return sum
        "#;
        assert_eq!(eval(src).unwrap(), Value::Int(55));
    }

    #[test]
    fn break_exits_loop_early() {
        let src = r#"
            local i: integer = 0
            while true do
              i = i + 1
              if i == 5 then
                break
              end
            end
            return i
        "#;
        assert_eq!(eval(src).unwrap(), Value::Int(5));
    }

    #[test]
    fn continue_skips_rest_of_iteration() {
        // Sum of odd numbers 1..=9 = 25.
        let src = r#"
            local i: integer = 0
            local sum: integer = 0
            while i < 10 do
              i = i + 1
              if i % 2 == 0 then
                continue
              end
              sum = sum + i
            end
            return sum
        "#;
        assert_eq!(eval(src).unwrap(), Value::Int(25));
    }

    // ── Phase 2: repeat / until ──────────────────────────────────────────────

    #[test]
    fn repeat_until_runs_at_least_once() {
        let src = r#"
            local i: integer = 0
            repeat
              i = i + 1
            until i >= 3
            return i
        "#;
        assert_eq!(eval(src).unwrap(), Value::Int(3));
    }

    // ── Phase 2: numeric for ─────────────────────────────────────────────────

    #[test]
    fn numeric_for_default_step() {
        let src = r#"
            local sum: integer = 0
            for i: integer = 1, 5 do
              sum = sum + i
            end

            return sum
        "#;
        assert_eq!(eval(src).unwrap(), Value::Int(15));
    }

    #[test]
    fn numeric_for_negative_step() {
        let src = r#"
            local sum: integer = 0
            for i: integer = 10, 1, -1 do
              sum = sum + i
            end
            sum
        "#;
        assert_eq!(eval(src).unwrap(), Value::Int(55));
    }

    #[test]
    fn numeric_for_zero_step_errors() {
        let src = r#"
            for i: integer = 1, 5, 0 do
              break
            end
        "#;
        assert!(matches!(
            eval(src).unwrap_err(),
            RuntimeError::ZeroStep { .. }
        ));
    }

    // ── Phase 2: stray loop-control statements ───────────────────────────────

    #[test]
    fn break_outside_loop_errors() {
        assert!(matches!(
            eval("break").unwrap_err(),
            RuntimeError::LoopControlOutsideLoop { which: "break", .. }
        ));
    }

    // ── Phase 3: user-defined functions ────────────────────────────────────────

    #[test]
    fn calls_named_function() {
        let src = r#"
            fn add(a: integer, b: integer): integer
              return a + b
            end
            add(2, 3)
        "#;
        assert_eq!(eval(src).unwrap(), Value::Int(5));
    }

    #[test]
    fn function_with_no_explicit_return_is_nil() {
        let src = r#"
            fn nothing(): nil
            end
            nothing()
        "#;
        assert_eq!(eval(src).unwrap(), Value::Nil);
    }

    #[test]
    fn recursion_factorial() {
        let src = r#"
            fn fact(n: integer): integer
              if n <= 1 then
                return 1
              end
              return n * fact(n - 1)
            end
            fact(6)
        "#;
        assert_eq!(eval(src).unwrap(), Value::Int(720));
    }

    #[test]
    fn closures_capture_lexical_scope() {
        // `make_adder` returns a lambda that adds its captured `n`.
        let src = r#"
            fn make_adder(n: integer): fn(integer): integer
              return (x: integer) => x + n
            end
            local add10 = make_adder(10)
            local add100 = make_adder(100)
            add10(5) + add100(5)
        "#;
        assert_eq!(eval(src).unwrap(), Value::Int(120));
    }

    #[test]
    fn default_parameter_value() {
        let src = r#"
            fn greet(name: string = "world"): string
              return "hello, " .. name
            end
            greet()
        "#;
        assert_eq!(
            eval(src).unwrap(),
            Value::Str(Rc::new("hello, world".into()))
        );
    }

    #[test]
    fn missing_argument_errors() {
        let src = r#"
            fn add(a: integer, b: integer): integer
              return a + b
            end
            add(1)
        "#;
        assert!(matches!(
            eval(src).unwrap_err(),
            RuntimeError::TypeError { .. }
        ));
    }

    #[test]
    fn lambda_assigned_and_called() {
        let src = r#"
            local sq = (x: integer) => x * x
            sq(9)
        "#;
        assert_eq!(eval(src).unwrap(), Value::Int(81));
    }

    #[test]
    fn higher_order_function() {
        let src = r#"
            fn apply(f: fn(integer): integer, x: integer): integer
              return f(x)
            end
            apply((n: integer) => n + 1, 41)
        "#;
        assert_eq!(eval(src).unwrap(), Value::Int(42));
    }

    #[test]
    fn function_type_name_is_function() {
        let src = r#"
            fn id(x: integer): integer return x end
            type(id)
        "#;
        assert_eq!(eval(src).unwrap(), Value::Str(Rc::new("function".into())));
    }

    #[test]
    fn calling_non_callable_errors() {
        let src = r#"
            local x: integer = 5
            x()
        "#;
        assert!(matches!(
            eval(src).unwrap_err(),
            RuntimeError::TypeError { .. }
        ));
    }

    // ────────────────────────────────────────────────────────────────────────
    // Specification-driven tests for the rest of the language.
    //
    // Every test below mirrors a feature described in README.md. They are
    // intentionally written *before* the supporting code so we can run
    // them and see exactly which features the interpreter still needs to
    // grow into. Tests that depend on features not yet implemented will
    // currently fail — that's the point.
    //
    // `try_eval` swallows lex / parse / runtime failures into a single
    // `Result<Value, String>` so failures show up as clean assertion
    // messages instead of panics in unrelated spots.
    // ────────────────────────────────────────────────────────────────────────

    fn try_eval(src: &str) -> Result<Value, String> {
        let toks = Lexer::new(src)
            .tokenize()
            .map_err(|e| format!("lex error: {e:?}"))?;
        let module = parse(toks).map_err(|e| format!("parse error: {e:?}"))?;
        run(&module).map_err(|e| format!("runtime error: {e:?}"))
    }

    fn assert_int(src: &str, expected: i64) {
        match try_eval(src) {
            Ok(Value::Int(n)) => assert_eq!(n, expected, "src: {src}"),
            Ok(other) => panic!("expected Int({expected}), got {other:?} — src: {src}"),
            Err(e) => panic!("evaluation failed ({e}) — src: {src}"),
        }
    }
    fn assert_float(src: &str, expected: f64) {
        match try_eval(src) {
            Ok(Value::Float(f)) => assert!((f - expected).abs() < 1e-9, "got {f}, want {expected}"),
            Ok(other) => panic!("expected Float({expected}), got {other:?} — src: {src}"),
            Err(e) => panic!("evaluation failed ({e}) — src: {src}"),
        }
    }
    fn assert_str(src: &str, expected: &str) {
        match try_eval(src) {
            Ok(Value::Str(s)) => assert_eq!(&*s, expected, "src: {src}"),
            Ok(other) => panic!("expected Str({expected:?}), got {other:?} — src: {src}"),
            Err(e) => panic!("evaluation failed ({e}) — src: {src}"),
        }
    }
    fn assert_bool(src: &str, expected: bool) {
        match try_eval(src) {
            Ok(Value::Bool(b)) => assert_eq!(b, expected, "src: {src}"),
            Ok(other) => panic!("expected Bool({expected}), got {other:?} — src: {src}"),
            Err(e) => panic!("evaluation failed ({e}) — src: {src}"),
        }
    }
    fn assert_nil(src: &str) {
        match try_eval(src) {
            Ok(Value::Nil) => {}
            Ok(other) => panic!("expected Nil, got {other:?} — src: {src}"),
            Err(e) => panic!("evaluation failed ({e}) — src: {src}"),
        }
    }
    fn assert_errs(src: &str) {
        if let Ok(v) = try_eval(src) {
            panic!("expected error, got {v:?} — src: {src}");
        }
    }

    // ── §Types & §Casting ───────────────────────────────────────────────────

    mod casts {
        use super::*;

        #[test]
        fn int_truncates_positive_toward_zero() {
            assert_int("int(7.9)", 7);
        }
        #[test]
        fn int_truncates_negative_toward_zero() {
            // README: "truncation not rounding"; -3.7 → -3, NOT -4.
            assert_int("int(-3.7)", -3);
        }
        #[test]
        fn int_of_whole_float() {
            assert_int("int(10.0)", 10);
        }
        #[test]
        fn float_promotes_integer() {
            assert_float("float(5)", 5.0);
        }
        #[test]
        fn cast_enables_mixed_arithmetic() {
            // README example: `float(health) - dmg`.
            let src = r#"
                local health: integer = 100
                local dmg: float = 10.5
                float(health) - dmg
            "#;
            assert_float(src, 89.5);
        }
        #[test]
        fn cast_back_to_integer() {
            let src = r#"
                local health: integer = 100
                local dmg: float = 10.5
                health - int(dmg)
            "#;
            assert_int(src, 90);
        }
        #[test]
        fn int_of_string_errors() {
            assert_errs(r#"int("abc")"#);
        }
        #[test]
        fn float_of_string_errors() {
            assert_errs(r#"float("abc")"#);
        }
        #[test]
        fn int_with_no_args_errors() {
            assert_errs("int()");
        }
    }

    // ── §Tables ─────────────────────────────────────────────────────────────

    mod tables {
        use super::*;

        #[test]
        fn empty_table_literal_has_zero_length() {
            assert_int("local t = {}\n#t", 0);
        }
        #[test]
        fn array_literal_with_elements() {
            assert_int("local t = {10, 20, 30}\nt[2]", 20);
        }
        #[test]
        fn length_of_populated_table() {
            assert_int("#{1,2,3,4,5}", 5);
        }
        #[test]
        fn out_of_bounds_index_returns_nil() {
            assert_nil("local t = {1,2,3}\nt[99]");
        }
        #[test]
        fn zero_index_returns_nil() {
            assert_nil("local t = {1,2,3}\nt[0]");
        }
        #[test]
        fn negative_index_returns_nil() {
            assert_nil("local t = {1,2,3}\nt[-1]");
        }
        #[test]
        fn index_assignment_updates_element() {
            assert_int("local t = {1,2,3}\nt[2] = 99\nt[2]", 99);
        }
        #[test]
        fn append_idiom_grows_table() {
            // README pattern: `result[#result + 1] = item`.
            let src = r#"
                local t = {1, 2}
                t[#t + 1] = 3
                t[#t + 1] = 4
                #t
            "#;
            assert_int(src, 4);
        }
        #[test]
        fn nested_table_access() {
            assert_int("local g = {{1,2,3},{4,5,6}}\ng[2][3]", 6);
        }
        #[test]
        fn tables_have_reference_identity() {
            // Aliased tables share storage.
            let src = r#"
                local a = {1,2,3}
                local b = a
                b[1] = 99
                a[1]
            "#;
            assert_int(src, 99);
        }
        #[test]
        fn tables_passed_to_fn_are_shared() {
            let src = r#"
                fn modify(t: table<integer>): nil
                    t[1] = 42
                end
                local t = {1, 2, 3}
                modify(t)
                t[1]
            "#;
            assert_int(src, 42);
        }
        #[test]
        fn indexing_non_table_errors() {
            assert_errs("local x = 5\nx[1]");
        }
        #[test]
        fn index_assign_on_non_table_errors() {
            assert_errs("local x = 5\nx[1] = 9");
        }
        #[test]
        fn type_of_table_is_table() {
            assert_str("type({1,2,3})", "table");
        }
    }

    // ── §Loops (for-in) ─────────────────────────────────────────────────────

    mod for_in {
        use super::*;

        #[test]
        fn one_var_iterates_values() {
            let src = r#"
                local sum: integer = 0
                for v: integer in {1, 2, 3, 4} do
                    sum = sum + v
                end
                sum
            "#;
            assert_int(src, 10);
        }
        #[test]
        fn two_vars_iterate_index_and_value() {
            let src = r#"
                local sum: integer = 0
                for i: integer, v: integer in {10, 20, 30} do
                    sum = sum + i + v
                end
                sum
            "#;
            // (1+10)+(2+20)+(3+30) = 66
            assert_int(src, 66);
        }
        #[test]
        fn empty_table_runs_body_zero_times() {
            let src = r#"
                local hit: integer = 0
                for v: integer in {} do
                    hit = hit + 1
                end
                hit
            "#;
            assert_int(src, 0);
        }
        #[test]
        fn break_inside_for_in() {
            let src = r#"
                local last: integer = 0
                for v: integer in {1,2,3,4,5} do
                    if v == 3 then break end
                    last = v
                end
                last
            "#;
            assert_int(src, 2);
        }
        #[test]
        fn continue_inside_for_in() {
            let src = r#"
                local sum: integer = 0
                for v: integer in {1,2,3,4,5} do
                    if v % 2 == 0 then continue end
                    sum = sum + v
                end
                sum
            "#;
            assert_int(src, 9);
        }
    }

    // ── §Functions: multiple return values ──────────────────────────────────

    mod multi_return {
        use super::*;

        #[test]
        fn multi_return_destructured() {
            let src = r#"
                fn pair(): (integer, integer)
                    return 7, 9
                end
                local a: integer, b: integer = pair()
                a + b
            "#;
            assert_int(src, 16);
        }
        #[test]
        fn multi_return_through_named_function_call() {
            let src = r#"
                fn minMax(items: table<integer>) -> (integer, integer)
                    local lo: integer = items[1]
                    local hi: integer = items[1]
                    for v: integer in items do
                        if v < lo then lo = v end
                        if v > hi then hi = v end
                    end
                    return lo, hi
                end
                local lo: integer, hi: integer = minMax({3,1,7,2,9})
                lo * 100 + hi
            "#;
            assert_int(src, 109);
        }
        #[test]
        fn multi_return_min_max() {
            // README example.
            let src = r#"
                fn minMax(items: table<integer>): (integer, integer)
                    local lo: integer = items[1]
                    local hi: integer = items[1]
                    for v: integer in items do
                        if v < lo then lo = v end
                        if v > hi then hi = v end
                    end
                    return lo, hi
                end
                local lo: integer, hi: integer = minMax({3,1,7,2,9})
                hi - lo
            "#;
            assert_int(src, 8);
        }
        #[test]
        fn single_binding_of_multi_return_takes_first() {
            let src = r#"
                fn pair(): (integer, integer)
                    return 11, 22
                end
                local x: integer = pair()
                x
            "#;
            assert_int(src, 11);
        }
    }

    // ── §Functions: named parameters ────────────────────────────────────────

    mod named_params {
        use super::*;

        #[test]
        fn call_with_named_arguments() {
            let src = r#"
                fn setup(width: integer, height: integer, title: string): string
                    return title .. " " .. width .. "x" .. height
                end

                setup(width: 1920, height: 1080, title: "Game")
            "#;
            assert_str(src, "Game 1920x1080");
        }
        #[test]
        fn named_args_in_any_order() {
            let src = r#"
                fn make(a: integer, b: integer, c: integer): integer
                    return a * 100 + b * 10 + c
                end
                make(c: 3, a: 1, b: 2)
            "#;
            assert_int(src, 123);
        }
        #[test]
        fn named_with_default() {
            let src = r#"
                fn fmt(x: integer, suffix: string = "px"): string
                    return x .. suffix
                end
                fmt(x: 16)
            "#;
            assert_str(src, "16px");
        }
    }

    // ── §Functions: variadic ────────────────────────────────────────────────

    mod variadic {
        use super::*;

        #[test]
        fn variadic_sums_integers() {
            // README example.
            let src = r#"
                fn sum(...values: integer): integer
                    local total: integer = 0
                    for v: integer in values do
                        total = total + v
                    end
                    return total
                end
                sum(1, 2, 3, 4, 5)
            "#;
            assert_int(src, 15);
        }
        #[test]
        fn variadic_with_zero_extras() {
            let src = r#"
                fn sum(...values: integer): integer
                    local total: integer = 0
                    for v: integer in values do total = total + v end
                    return total
                end
                sum()
            "#;
            assert_int(src, 0);
        }
        #[test]
        fn variadic_after_fixed_param() {
            let src = r#"
                fn label(prefix: string, ...vs: integer): string
                    local s: string = prefix
                    for v: integer in vs do s = s .. " " .. v end
                    return s
                end
                label("ids:", 7, 8, 9)
            "#;
            assert_str(src, "ids: 7 8 9");
        }
    }

    // ── §Functions: piping with `then` ──────────────────────────────────────

    mod piping {
        use super::*;

        #[test]
        fn pipe_threads_value_into_first_arg() {
            let src = r#"
                fn double(x: integer): integer return x * 2 end
                fn inc(x: integer): integer return x + 1 end
                local r: integer = 10 then double() then inc()
                r
            "#;
            assert_int(src, 21);
        }
        #[test]
        fn pipe_with_extra_args() {
            let src = r#"
                fn add(a: integer, b: integer): integer return a + b end
                local r: integer = 1 then add(41)
                r
            "#;
            assert_int(src, 42);
        }
    }

    // ── §Classes & §Inheritance ─────────────────────────────────────────────

    mod classes {
        use super::*;

        #[test]
        fn construct_and_read_field() {
            let src = r#"
                class Point
                    x: integer
                    y: integer
                    fn init(x: integer, y: integer)
                        self.x = x
                        self.y = y
                    end
                end
                local p: Point = new Point(3, 4)
                p.x + p.y
            "#;
            assert_int(src, 7);
        }
        #[test]
        fn method_call_with_self() {
            let src = r#"
                class Greeter
                    name: string
                    fn init(name: string)
                        self.name = name
                    end
                    fn greet(self): string
                        return "hi " .. self.name
                    end
                end
                local g: Greeter = new Greeter("ada")
                g:greet()
            "#;
            assert_str(src, "hi ada");
        }
        #[test]
        fn method_can_mutate_self() {
            let src = r#"
                class Counter
                    n: integer
                    fn init()
                        self.n = 0
                    end
                    fn tick(self): nil
                        self.n = self.n + 1
                    end
                end
                local c: Counter = new Counter()
                c:tick() c:tick() c:tick()
                c.n
            "#;
            assert_int(src, 3);
        }
        #[test]
        fn static_field_access() {
            let src = r#"
                class Player
                    static maxHealth: integer = 100
                end
                Player.maxHealth
            "#;
            assert_int(src, 100);
        }
        #[test]
        fn static_method_call() {
            let src = r#"
                class Player
                    static maxHealth: integer = 100
                    static fn getMax(): integer
                        return Player.maxHealth
                    end
                end
                Player.getMax()
            "#;
            assert_int(src, 100);
        }
        #[test]
        fn static_field_is_shared_and_mutable() {
            let src = r#"
                class Player
                    static maxHealth: integer = 100
                end
                Player.maxHealth = 200
                Player.maxHealth
            "#;
            assert_int(src, 200);
        }
        #[test]
        fn instance_identity_is_per_object() {
            let src = r#"
                class Box
                    v: integer
                    fn init(v: integer)
                        self.v = v
                    end
                end
                local a: Box = new Box(1)
                local b: Box = new Box(1)
                a == b
            "#;
            assert_bool(src, false);
        }
        #[test]
        fn field_default_value() {
            // README shows `static maxHealth: integer = 100` and class fields
            // typically initialized in the constructor; but README also uses
            // `static score: integer = 0` in a constructor signature. Test
            // that a static default is observable before any instance exists.
            let src = r#"
                class Cfg
                    static debug: boolean = true
                end
                Cfg.debug
            "#;
            assert_bool(src, true);
        }
        #[test]
        fn type_of_instance_is_instance() {
            // Saule uses class names as runtime type tags; `type(p)` should
            // yield the class name (or at least something non-empty).
            let src = r#"
                class Foo end
                type(new Foo())
            "#;
            // We only assert it's a string of "Foo" or similar; the exact
            // format will be locked once classes are implemented.
            assert_str(src, "Foo");
        }
    }

    mod inheritance {
        use super::*;

        #[test]
        fn child_inherits_parent_method() {
            let src = r#"
                class Entity
                    name: string
                    fn init(name: string)
                        self.name = name
                    end
                    fn getName(self): string return self.name end
                end
                class Player extends Entity
                    fn init(name: string)
                        self.super(name)
                    end
                end
                local p: Player = new Player("arthur")
                p:getName()
            "#;
            assert_str(src, "arthur");
        }
        #[test]
        fn super_calls_parent_constructor() {
            let src = r#"
                class A
                    a: integer
                    fn init(a: integer)
                        self.a = a
                    end
                end
                class B extends A
                    b: integer
                    fn init(a: integer, b: integer)
                        self.super(a)
                        self.b = b
                    end
                end
                local x: B = new B(10, 20)
                x.a + x.b
            "#;
            assert_int(src, 30);
        }
        #[test]
        fn child_overrides_parent_method() {
            let src = r#"
                class A
                    fn label(self): string return "A" end
                end
                class B extends A
                    fn label(self): string return "B" end
                end
                (new B()):label()
            "#;
            assert_str(src, "B");
        }
    }

    // ── §Interfaces ─────────────────────────────────────────────────────────

    mod interfaces {
        use super::*;

        #[test]
        fn class_implements_interface_at_runtime() {
            // Interfaces are erased at runtime; calling the contract method
            // through an interface-typed local must still dispatch correctly.
            let src = r#"
                interface Greetable
                    fn greet(self): string
                end
                class Person implements Greetable
                    name: string
                    fn init(n: string)
                        self.name = n
                    end
                    fn greet(self): string return "hello " .. self.name end
                end
                local g: Greetable = new Person("rust")
                g:greet()
            "#;
            assert_str(src, "hello rust");
        }
    }

    // ── §Enums ──────────────────────────────────────────────────────────────

    mod enums {
        use super::*;

        #[test]
        fn enum_variant_equality() {
            let src = r#"
                enum Direction
                    North
                    South
                    East
                    West
                end
                Direction.North == Direction.North
            "#;
            assert_bool(src, true);
        }
        #[test]
        fn enum_variants_are_distinct() {
            let src = r#"
                enum Direction
                    North
                    South
                end
                Direction.North == Direction.South
            "#;
            assert_bool(src, false);
        }
        #[test]
        fn valued_enum_exposes_value() {
            let src = r#"
                enum Status
                    Alive = "alive"
                    Dead = "dead"
                end
                Status.Alive.value
            "#;
            assert_str(src, "alive");
        }
        #[test]
        fn enum_method_dispatch() {
            // README example.
            let src = r#"
                enum Status
                    Alive = "alive"
                    Dead = "dead"
                    fn describe(self): string
                        return "Status is: " .. self.value
                    end
                end
                (Status.Alive):describe()
            "#;
            assert_str(src, "Status is: alive");
        }
    }

    // ── §Null Safety ────────────────────────────────────────────────────────

    mod null_safety {
        use super::*;

        #[test]
        fn safe_member_on_nil_returns_nil() {
            let src = r#"
                class P
                    name: string
                    fn init(n: string)
                        self.name = n
                    end
                end
                local p: P? = nil
                p?.name
            "#;
            assert_nil(src);
        }
        #[test]
        fn safe_member_on_value_returns_field() {
            let src = r#"
                class P
                    name: string
                    fn init(n: string)
                        self.name = n
                    end
                end
                local p: P? = new P("ada")
                p?.name
            "#;
            assert_str(src, "ada");
        }
        #[test]
        fn null_coalesce_with_nil() {
            let src = r#"
                local x: string? = nil
                x ?? "fallback"
            "#;
            assert_str(src, "fallback");
        }
        #[test]
        fn null_coalesce_with_value() {
            let src = r#"
                local x: string? = "real"
                x ?? "fallback"
            "#;
            assert_str(src, "real");
        }
        #[test]
        fn force_unwrap_on_value_returns_value() {
            let src = r#"
                local x: string? = "ok"
                x!
            "#;
            assert_str(src, "ok");
        }
        #[test]
        fn force_unwrap_on_nil_errors() {
            assert_errs("local x: string? = nil\nx!");
        }
        #[test]
        fn combined_safe_call_and_coalesce() {
            let src = r#"
                class P
                    name: string
                    fn init(n: string)
                        self.name = n
                    end
                    fn getName(self): string return self.name end
                end
                local p: P? = nil
                local n: string = p?:getName() ?? "Unknown"
                n
            "#;
            assert_str(src, "Unknown");
        }
    }

    // ── §Error Handling ─────────────────────────────────────────────────────

    mod error_handling {
        use super::*;

        #[test]
        fn try_catches_thrown_string() {
            let src = r#"
                local caught: string = ""
                try
                    throw "boom"
                catch e: string
                    caught = e
                end
                caught
            "#;
            assert_str(src, "boom");
        }
        #[test]
        fn try_body_completes_without_throw() {
            let src = r#"
                local x: integer = 0
                try
                    x = 42
                catch e: string
                    x = -1
                end
                x
            "#;
            assert_int(src, 42);
        }
        #[test]
        fn nested_try_catch() {
            let src = r#"
                local outer: string = "none"
                try
                    try
                        throw "inner"
                    catch e: string
                        throw "wrapped: " .. e
                    end
                catch e: string
                    outer = e
                end
                outer
            "#;
            assert_str(src, "wrapped: inner");
        }
        #[test]
        fn uncaught_throw_bubbles_as_error() {
            assert_errs(r#"throw "kaboom""#);
        }
        #[test]
        fn throw_from_inside_function_caught_outside() {
            let src = r#"
                fn bad(): nil
                    throw "no good"
                end
                local msg: string = ""
                try
                    bad()
                catch e: string
                    msg = e
                end
                msg
            "#;
            assert_str(src, "no good");
        }
    }

    // ── §Loops (numeric-for `continue`/`break` already covered earlier) ─────

    mod loops {
        use super::*;

        #[test]
        fn numeric_for_with_step_5() {
            let src = r#"
                local sum: integer = 0
                for i: integer = 0, 20, 5 do
                    sum = sum + i
                end
                sum
            "#;
            // 0+5+10+15+20 = 50
            assert_int(src, 50);
        }
        #[test]
        fn for_combined_break_and_continue() {
            // README example mixing both.
            let src = r#"
                local seen: string = ""
                for i: integer = 1, 10 do
                    if i == 5 then continue end
                    if i == 8 then break end
                    seen = seen .. i
                end
                seen
            "#;
            assert_str(src, "123467");
        }
    }

    // ── §Operators: full reference table ────────────────────────────────────

    mod operators {
        use super::*;

        #[test]
        fn arithmetic_full() {
            assert_int("1 + 2", 3);
            assert_int("10 - 4", 6);
            assert_int("3 * 7", 21);
            assert_int("20 / 4", 5);
            assert_int("17 % 5", 2);
        }
        #[test]
        fn comparison_full() {
            assert_bool("1 == 1", true);
            assert_bool("1 != 2", true);
            assert_bool("1 < 2", true);
            assert_bool("2 <= 2", true);
            assert_bool("3 > 2", true);
            assert_bool("3 >= 3", true);
        }
        #[test]
        fn boolean_logic_full() {
            assert_bool("true and false", false);
            assert_bool("true or false", true);
            assert_bool("not false", true);
        }
        #[test]
        fn string_ops() {
            assert_str(r#""a" .. "b" .. "c""#, "abc");
            assert_int(r#"#"hello""#, 5);
        }
        #[test]
        fn null_safety_ops_list() {
            // Already covered individually; this is a smoke check that all
            // three operators are accepted in one expression.
            let src = r#"
                local x: string? = nil
                x?.length ?? "n/a"
            "#;
            assert_str(src, "n/a");
        }
    }
}

