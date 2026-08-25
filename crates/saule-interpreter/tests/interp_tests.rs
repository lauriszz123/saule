//! Tests moved out of src/lib.rs.
use saule_interpreter::value::SauleStr;
use saule_interpreter::{PipelineError, RuntimeError, Value, check_and_run};
use saule_lexer::Lexer;
use saule_parser::parse;
use saule_semantic::SemanticError;

fn eval(src: &str) -> Result<Value, PipelineError> {
    let toks = Lexer::new(src).tokenize().expect("lex");
    let module = parse(toks).expect("parse");
    check_and_run(&module)
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
        PipelineError::Typeck(saule_typeck::TypeCheckError::NumericMix { .. })
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
        Value::Str(SauleStr::new("foobar".into()))
    );
    assert_eq!(eval(r#"#"hello""#).unwrap(), Value::Int(5));
}

#[test]
fn comparison_and_logic() {
    assert_eq!(eval("1 < 2 and 3 >= 3").unwrap(), Value::Bool(true));
    assert_eq!(
        eval(r#"nil or "fallback""#).unwrap(),
        Value::Str(SauleStr::new("fallback".into()))
    );
}

#[test]
fn null_coalescing() {
    assert_eq!(
        eval(r#"nil ?? "x""#).unwrap(),
        Value::Str(SauleStr::new("x".into()))
    );
    assert_eq!(
        eval(r#""y" ?? "x""#).unwrap(),
        Value::Str(SauleStr::new("y".into()))
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
        PipelineError::Semantic(SemanticError::UndefinedName { .. })
    ));
}

#[test]
fn division_by_zero() {
    assert!(matches!(
        eval("1 / 0").unwrap_err(),
        PipelineError::Runtime(RuntimeError::DivisionByZero { .. })
    ));
}

#[test]
fn type_builtin() {
    assert_eq!(
        eval("type(42)").unwrap(),
        Value::Str(SauleStr::new("integer".into()))
    );
    assert_eq!(
        eval(r#"type("hi")"#).unwrap(),
        Value::Str(SauleStr::new("string".into()))
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
        PipelineError::Semantic(SemanticError::AssignToUndeclared { .. })
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
        elseif n == 0 then
          kind = "zero"
        elseif n < 10 then
          kind = "small"
        else
          kind = "big"
        end
        kind
    "#;
    assert_eq!(eval(src).unwrap(), Value::Str(SauleStr::new("small".into())));
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
        PipelineError::Semantic(SemanticError::UndefinedName { .. })
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
        PipelineError::Runtime(RuntimeError::ZeroStep { .. })
    ));
}

// ── Phase 2: stray loop-control statements ───────────────────────────────

#[test]
fn break_outside_loop_errors() {
    assert!(matches!(
        eval("break").unwrap_err(),
        PipelineError::Semantic(SemanticError::LoopControlOutsideLoop { which: "break", .. })
    ));
}

// ── Phase 3: user-defined functions ────────────────────────────────────────

#[test]
fn calls_named_function() {
    let src = r#"
        fn add(a: integer, b: integer) -> integer
          return a + b
        end
        add(2, 3)
    "#;
    assert_eq!(eval(src).unwrap(), Value::Int(5));
}

#[test]
fn function_with_no_explicit_return_is_nil() {
    let src = r#"
        fn nothing() -> nil
        end
        nothing()
    "#;
    assert_eq!(eval(src).unwrap(), Value::Nil);
}

#[test]
fn recursion_factorial() {
    let src = r#"
        fn fact(n: integer) -> integer
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
        fn make_adder(n: integer) -> fn(integer) -> integer
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
        fn greet(name: string = "world") -> string
          return "hello, " .. name
        end
        greet()
    "#;
    assert_eq!(
        eval(src).unwrap(),
        Value::Str(SauleStr::new("hello, world".into()))
    );
}

#[test]
fn missing_argument_errors() {
    let src = r#"
        fn add(a: integer, b: integer) -> integer
          return a + b
        end
        add(1)
    "#;
    // Caught at the typeck phase now: `saule_typeck` validates direct
    // calls to top-level functions against their declared arity.
    assert!(matches!(
        eval(src).unwrap_err(),
        PipelineError::Typeck(saule_typeck::TypeCheckError::FunctionArity { .. })
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
        fn apply(f: fn(integer) -> integer, x: integer) -> integer
          return f(x)
        end
        apply((n: integer) => n + 1, 41)
    "#;
    assert_eq!(eval(src).unwrap(), Value::Int(42));
}

#[test]
fn function_type_name_is_function() {
    let src = r#"
        fn id(x: integer) -> integer return x end
        type(id)
    "#;
    assert_eq!(eval(src).unwrap(), Value::Str(SauleStr::new("function".into())));
}

#[test]
fn calling_non_callable_errors() {
    let src = r#"
        local x: integer = 5
        x()
    "#;
    assert!(matches!(
        eval(src).unwrap_err(),
        PipelineError::Runtime(RuntimeError::TypeError { .. })
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
    check_and_run(&module).map_err(|e| format!("{e:?}"))
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

// ── §Stdlib constants ───────────────────────────────────────────────────

/// `Math.huge`, `Os.sep`, `Io.stdout` and friends hold *values*, not
/// callables. They were registered as zero-arg native signatures, which
/// broke both spellings: bare use couldn't be typed (`UndeterminedType`)
/// and the call the signature invited died at runtime on a non-callable
/// float. These pin the constant down as a typed value.
mod stdlib_constants {
    use super::*;

    #[test]
    fn math_constants_are_typed_values() {
        assert_eq!(
            eval("local m: integer = Math.maxinteger\nm").unwrap(),
            Value::Int(i64::MAX)
        );
        assert_eq!(
            eval("local h: float = Math.huge\nh > 0.0").unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            eval("local p: float = Math.pi\np > 3.0 and p < 3.2").unwrap(),
            Value::Bool(true)
        );
    }

    #[test]
    fn os_and_project_constants_are_typed_values() {
        assert_eq!(
            eval("local s: string = Os.sep\n#s > 0").unwrap(),
            Value::Bool(true)
        );
        // `Project` is installed even in single-file mode (defaulted), so
        // its fields are unconditionally present and typed.
        assert_eq!(
            eval("local d: table<string> = Project.srcDirs\n#d >= 0").unwrap(),
            Value::Bool(true)
        );
    }

    /// The type is real, not a rubber stamp — a wrong annotation is caught.
    #[test]
    fn a_constant_is_checked_against_its_annotation() {
        assert!(matches!(
            eval("local h: integer = Math.huge\nh").unwrap_err(),
            PipelineError::Typeck(saule_typeck::TypeCheckError::AssignmentTypeMismatch { .. })
        ));
    }

    /// Calling one is reported as such rather than inferring `any` and
    /// surfacing as a baffling mismatch at the binding.
    #[test]
    fn calling_a_constant_is_rejected_with_a_clear_message() {
        let err = eval("local h: float = Math.huge()\nh").unwrap_err();
        assert!(
            matches!(
                err,
                PipelineError::Typeck(saule_typeck::TypeCheckError::CallOfConstant { .. })
            ),
            "got: {err:?}"
        );
    }

    /// Typos must still be caught — the constants table records member
    /// names too, so the unknown-member check keeps working.
    #[test]
    fn an_unknown_math_member_is_still_flagged() {
        assert!(matches!(
            eval("local x: float = Math.bogus\nx").unwrap_err(),
            PipelineError::Typeck(saule_typeck::TypeCheckError::UnknownMember { .. })
        ));
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
            fn modify(t: table<integer>)
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
            fn pair() -> (integer, integer)
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
            hi - lo
        "#;
        assert_int(src, 8);
    }
    #[test]
    fn single_binding_of_multi_return_takes_first() {
        let src = r#"
            fn pair() -> (integer, integer)
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
            fn setup(width: integer, height: integer, title: string) -> string
                return title .. " " .. width .. "x" .. height
            end

            setup(width: 1920, height: 1080, title: "Game")
        "#;
        assert_str(src, "Game 1920x1080");
    }
    #[test]
    fn named_args_in_any_order() {
        let src = r#"
            fn make(a: integer, b: integer, c: integer) -> integer
                return a * 100 + b * 10 + c
            end
            make(c: 3, a: 1, b: 2)
        "#;
        assert_int(src, 123);
    }
    #[test]
    fn named_with_default() {
        let src = r#"
            fn fmt(x: integer, suffix: string = "px") -> string
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
            fn sum(...values: integer) -> integer
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
            fn sum(...values: integer) -> integer
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
            fn label(prefix: string, ...vs: integer) -> string
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
            fn double(x: integer) -> integer return x * 2 end
            fn inc(x: integer) -> integer return x + 1 end
            local r: integer = when(10):double():inc()
            r
        "#;
        assert_int(src, 21);
    }
    #[test]
    fn pipe_with_extra_args() {
        let src = r#"
            fn add(a: integer, b: integer) -> integer return a + b end
            local r: integer = when(1):add(41)
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
            local p: Point = Point(3, 4)
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

                fn greet() -> string
                    return "hi " .. self.name
                end
            end
            local g: Greeter = Greeter("ada")
            g.greet()
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
                fn tick()
                    self.n = self.n + 1
                end
            end
            local c: Counter = Counter()
            c.tick() c.tick() c.tick()
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
                static fn getMax() -> integer
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
            local a: Box = Box(1)
            local b: Box = Box(1)
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
            type(Foo())
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
                fn getName() -> string return self.name end
            end
            class Player extends Entity
                fn init(name: string)
                    self.super(name)
                end
            end
            local p: Player = Player("arthur")
            p.getName()
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

            local x: B = B(10, 20)
            x.a + x.b
        "#;
        assert_int(src, 30);
    }
    #[test]
    fn child_overrides_parent_method() {
        let src = r#"
            class A
                fn label() -> string return "A" end
            end
            class B extends A
                fn label() -> string return "B" end
            end
            (B()).label()
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
                fn greet(self) -> string
            end
            class Person implements Greetable
                name: string
                fn init(n: string)
                    self.name = n
                end
                fn greet(self) -> string return "hello " .. self.name end
            end
            local g: Greetable = Person("rust")
            g.greet()
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
                fn describe(self) -> string
                    return "Status is: " .. self.value
                end
            end
            (Status.Alive).describe()
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
            local p: P? = P("ada")
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
                fn getName(self) -> string return self.name end
            end
            local p: P? = nil
            local n: string = p?.getName() ?? "Unknown"
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
            fn bad()
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
        assert_int("2 ^ 10", 1024);
    }
    #[test]
    fn pow_precedence_and_associativity() {
        // Right-associative and tighter than unary minus, like Lua.
        assert_int("2 ^ 3 ^ 2", 512);
        assert_int("-2 ^ 2", -4);
        assert_int("3 * 2 ^ 3", 24);
    }
    #[test]
    fn pow_negative_integer_exponent_errors() {
        // `integer ^ integer` stays an integer, so there is no answer to
        // give — better an error than a silently truncated 0.
        assert!(matches!(
            eval("2 ^ -1").unwrap_err(),
            PipelineError::Runtime(RuntimeError::TypeError { .. })
        ));
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

// ── §Operator overloading ───────────────────────────────────────────────

mod operator_overloading {
    use super::*;

    /// A class overloading `+`, `==`, `<`, and `tostring`. Reused by the
    /// tests below, which each append their own expression.
    fn with_money(expr: &str) -> String {
        format!(
            r#"
            class Money implements OpAdd, OpEq, OpCompare, OpToString
                local cents: integer
                fn init(c: integer)
                    self.cents = c
                end
                fn get() -> integer return self.cents end
                fn add(other: Money) -> Money return Money(self.cents + other.get()) end
                fn equals(other: Money) -> boolean return self.cents == other.get() end
                fn compare(other: Money) -> integer return self.cents - other.get() end
                fn toString() -> string return self.cents .. "c" end
            end
            {expr}
        "#
        )
    }

    #[test]
    fn add_returns_the_declared_type() {
        assert_int(&with_money("(Money(2) + Money(3)).get()"), 5);
    }

    #[test]
    fn equality_uses_the_overload_not_identity() {
        // Two distinct instances holding the same amount are equal, which
        // pointer identity would never report.
        assert_bool(&with_money("Money(7) == Money(7)"), true);
        assert_bool(&with_money("Money(7) != Money(8)"), true);
    }

    #[test]
    fn nil_never_reaches_the_overload() {
        // `equals(other: Money)` would fault on a `nil`; `m == nil` has to
        // stay the nullability check it looks like.
        let src = with_money("local m: Money? = nil\nm == nil");
        assert_bool(&src, true);
    }

    #[test]
    fn one_compare_drives_all_four_orderings() {
        assert_bool(&with_money("Money(1) < Money(2)"), true);
        assert_bool(&with_money("Money(2) <= Money(2)"), true);
        assert_bool(&with_money("Money(3) > Money(2)"), true);
        assert_bool(&with_money("Money(3) >= Money(3)"), true);
        assert_bool(&with_money("Money(3) < Money(2)"), false);
    }

    #[test]
    fn tostring_overload_drives_tostring_and_concat() {
        assert_str(&with_money(r#"tostring(Money(42))"#), "42c");
        assert_str(&with_money(r#""cost " .. Money(42)"#), "cost 42c");
    }

    #[test]
    fn overloads_are_inherited() {
        let src = r#"
            class Base implements OpAdd
                local n: integer
                fn init(n: integer)
                    self.n = n
                end
                fn get() -> integer return self.n end
                fn add(other: Base) -> Base return Base(self.n + other.get()) end
            end
            class Derived extends Base
                fn init(n: integer)
                    self.super(n)
                end
            end
            (Derived(4) + Base(5)).get()
        "#;
        assert_int(src, 9);
    }

    #[test]
    fn implements_clause_is_the_opt_in() {
        // The method alone isn't enough — without `implements OpAdd` the
        // operator stays a compile error.
        let src = r#"
            class Point
                local x: integer
                fn init(x: integer)
                    self.x = x
                end
                fn get() -> integer return self.x end
                fn add(other: Point) -> Point return Point(self.x + other.get()) end
            end
            Point(1) + Point(2)
        "#;
        assert!(matches!(
            eval(src).unwrap_err(),
            PipelineError::Typeck(saule_typeck::TypeCheckError::OperatorNotImplemented { .. })
        ));
    }

    #[test]
    fn arithmetic_dispatches_on_the_left_operand() {
        // `2 + money` must not quietly become `money + 2`.
        let src = with_money("2 + Money(1)");
        assert!(matches!(
            eval(&src).unwrap_err(),
            PipelineError::Typeck(saule_typeck::TypeCheckError::OperatorDispatchesOnLeft { .. })
        ));
    }

    #[test]
    fn operand_type_is_checked_against_the_method_signature() {
        let src = with_money("Money(1) + 5");
        assert!(matches!(
            eval(&src).unwrap_err(),
            PipelineError::Typeck(saule_typeck::TypeCheckError::OperatorOperandTypeMismatch { .. })
        ));
    }

    #[test]
    fn plain_instances_still_compare_by_identity() {
        // No `OpEq` means `==` keeps its built-in meaning rather than
        // becoming an error.
        let src = r#"
            class Plain
                local n: integer
                fn init(n: integer)
                    self.n = n
                end
            end
            local a: Plain = Plain(1)
            a == a
        "#;
        assert_bool(src, true);
    }
}

// ── Trailing blocks ─────────────────────────────────────────────────────────

#[test]
fn trailing_block_runs_as_the_final_argument() {
    let src = r#"
fn twice(body: fn() -> nil) -> nil
    body()
    body()
end

local n: integer = 0
twice()
do
    n = n + 1
end
n
"#;
    assert_eq!(eval(src).unwrap(), Value::Int(2));
}

#[test]
fn trailing_block_receives_parameters() {
    let src = r#"
fn each(items: table<integer>, body: fn(integer) -> nil) -> nil
    for _, v in items do
        body(v)
    end
end

local total: integer = 0
each({1, 2, 3})
do (v)
    total = total + v
end
total
"#;
    assert_eq!(eval(src).unwrap(), Value::Int(6));
}

#[test]
fn trailing_block_params_infer_from_the_parameter_type() {
    // `v` is a real `integer` inside the block, so a string op on it is an
    // error rather than a silent `any`.
    let src = r#"
fn each(items: table<integer>, body: fn(integer) -> nil) -> nil
    for _, v in items do
        body(v)
    end
end

each({1})
do (v)
    print(#v)
end
"#;
    assert!(eval(src).is_err());
}

#[test]
fn trailing_block_follows_named_arguments() {
    let src = r#"
fn repeated(times: integer, body: fn() -> nil) -> nil
    for _ = 1, times do
        body()
    end
end

local n: integer = 0
repeated(times: 3)
do
    n = n + 1
end
n
"#;
    assert_eq!(eval(src).unwrap(), Value::Int(3));
}

#[test]
fn trailing_block_closes_over_its_environment() {
    let src = r#"
fn apply(body: fn(integer) -> integer) -> integer
    return body(10)
end

local factor: integer = 3
apply()
do (n) -> integer
    return n * factor
end
"#;
    assert_eq!(eval(src).unwrap(), Value::Int(30));
}

#[test]
fn trailing_block_nests() {
    let src = r#"
fn group(label: string, body: fn() -> nil) -> nil
    body()
end

local log: string = ""
group("outer")
do
    log = log .. "a"
    group("inner")
    do
        log = log .. "b"
    end
end
log
"#;
    match eval(src).unwrap() {
        Value::Str(s) => assert_eq!(&*s, "ab"),
        v => panic!("expected string, got {v:?}"),
    }
}

#[test]
fn trailing_block_arity_is_checked() {
    let src = r#"
fn once(body: fn() -> nil) -> nil
    body()
end

once()
do (extra)
    print(extra)
end
"#;
    assert!(eval(src).is_err());
}

#[test]
fn trailing_block_binds_to_the_last_parameter_over_a_default() {
    // `spacing` sits between the named argument and the block. Binding the
    // block to the next free slot would put it in `spacing` and report `body`
    // missing; it belongs to the last parameter.
    let src = r#"
fn panel(title: string, spacing: integer = 7, body: fn() -> nil) -> nil
    body()
    print(title .. ":" .. spacing)
end

local out: string = ""
panel(title: "Stats")
do
    out = out .. "child "
end
out
"#;
    match eval(src).unwrap() {
        Value::Str(s) => assert_eq!(&*s, "child "),
        v => panic!("expected string, got {v:?}"),
    }
}

#[test]
fn trailing_block_binds_to_the_last_parameter_with_positional_args() {
    let src = r#"
fn panel(title: string, spacing: integer = 0, body: fn() -> integer) -> integer
    return body() + spacing
end

panel("Stats", 2)
do
    return 10
end
"#;
    assert_eq!(eval(src).unwrap(), Value::Int(12));
}

#[test]
fn trailing_block_skips_a_non_callback_parameter_that_follows_the_callback() {
    // `enabled` is the last parameter but cannot hold a block; the callback
    // is `onSelected`. Binding by position alone put the function in
    // `enabled` and left `onSelected` at its default, so the block never ran
    // and the widget silently did nothing.
    let src = r#"
fn menuItem(label: string = "", onSelected: fn() -> nil = () => nil, enabled: boolean = true) -> string
    onSelected()
    return label .. ":" .. tostring(enabled)
end

menuItem("Open")
do
    print("chosen")
end
"#;
    match eval(src).unwrap() {
        Value::Str(s) => assert_eq!(&*s, "Open:true"),
        v => panic!("expected string, got {v:?}"),
    }
}

#[test]
fn trailing_block_takes_the_callback_slot_after_a_named_argument() {
    let src = r#"
fn menuItem(label: string = "", onSelected: fn() -> nil = () => nil, enabled: boolean = true) -> boolean
    onSelected()
    return enabled
end

menuItem(label: "Open")
do
    print("chosen")
end
"#;
    assert_eq!(eval(src).unwrap(), Value::Bool(true));
}

#[test]
fn trailing_block_falls_through_when_the_callback_slot_is_taken() {
    // The positional `() => nil` already owns `onSelected`, so the block has
    // nowhere sensible to go: it lands on the last parameter, and a `boolean`
    // slot handed a function is a mismatch the checker reports rather than
    // one the callback rule quietly absorbs.
    let src = r#"
fn menuItem(label: string = "", onSelected: fn() -> nil = () => nil, enabled: boolean = true) -> boolean
    onSelected()
    return enabled
end

menuItem("Open", () => nil)
do
    print("chosen")
end
"#;
    assert!(eval(src).is_err());
}

#[test]
fn trailing_block_duplicating_a_named_last_parameter_is_an_error() {
    let src = r#"
fn view(spacing: integer, body: fn() -> nil) -> nil
    body()
end

view(spacing: 1, body: fn() end)
do
    print(1)
end
"#;
    assert!(eval(src).is_err());
}

// ── Compound assignment ──────────────────────────────────────────────────

#[test]
fn compound_assignment_arithmetic() {
    assert_eq!(eval("local n = 10\nn += 5\nn").unwrap(), Value::Int(15));
    assert_eq!(eval("local n = 10\nn -= 5\nn").unwrap(), Value::Int(5));
    assert_eq!(eval("local n = 10\nn *= 5\nn").unwrap(), Value::Int(50));
    assert_eq!(eval("local n = 10\nn /= 5\nn").unwrap(), Value::Int(2));
    assert_eq!(eval("local n = 10\nn %= 4\nn").unwrap(), Value::Int(2));
    assert_eq!(eval("local n = 2\nn ^= 10\nn").unwrap(), Value::Int(1024));
}

#[test]
fn compound_assignment_concat() {
    assert_eq!(
        eval("local s = \"foo\"\ns ..= \"bar\"\ns").unwrap(),
        Value::Str(SauleStr::new("foobar".into()))
    );
}

#[test]
fn compound_assignment_rhs_is_the_whole_expression() {
    // `p *= 3 + 4` is `p * 7`, not `(p * 3) + 4`.
    assert_eq!(eval("local p = 2\np *= 3 + 4\np").unwrap(), Value::Int(14));
}

#[test]
fn compound_assignment_updates_a_table_element() {
    assert_eq!(
        eval("local t = {10, 20}\nt[2] += 5\nt[2]").unwrap(),
        Value::Int(25)
    );
}

#[test]
fn compound_assignment_updates_an_instance_field() {
    let src = r#"
        class Counter
            n: integer
            fn init()
                self.n = 0
            end
            fn bump()
                self.n += 3
            end
        end
        local c = Counter()
        c.bump()
        c.n += 1
        c.n
    "#;
    assert_eq!(eval(src).unwrap(), Value::Int(4));
}

#[test]
fn compound_assignment_evaluates_an_index_target_once() {
    // The whole reason `Stmt::CompoundAssign` exists rather than parse-time
    // desugaring to `t[i()] = t[i()] + 1`: a side-effecting subscript must
    // run exactly once.
    let src = r#"
        local calls: integer = 0
        fn idx() -> integer
            calls += 1
            return 1
        end
        local t = {10}
        t[idx()] += 5
        calls
    "#;
    assert_eq!(eval(src).unwrap(), Value::Int(1));
}

#[test]
fn compound_assignment_evaluates_a_member_receiver_once() {
    let src = r#"
        class Box
            n: integer
            fn init()
                self.n = 0
            end
        end
        local calls: integer = 0
        local shared = Box()
        fn get() -> Box
            calls += 1
            return shared
        end
        get().n += 7
        calls * 100 + shared.n
    "#;
    // One call to `get()`, and the update landed on the shared instance.
    assert_eq!(eval(src).unwrap(), Value::Int(107));
}

#[test]
fn compound_assignment_dispatches_to_an_operator_overload() {
    let src = r#"
        class Vec implements OpAdd<Vec, Vec>
            x: integer
            fn init(x: integer)
                self.x = x
            end
            fn add(other: Vec) -> Vec
                return Vec(self.x + other.x)
            end
        end
        local v = Vec(1)
        v += Vec(10)
        v.x
    "#;
    assert_eq!(eval(src).unwrap(), Value::Int(11));
}

#[test]
fn compound_assignment_to_undeclared_is_rejected() {
    assert!(matches!(
        eval("zzz += 1").unwrap_err(),
        PipelineError::Semantic(SemanticError::AssignToUndeclared { .. })
    ));
}

// ── Bitwise operators ────────────────────────────────────────────────────

#[test]
fn bitwise_and_or_xor() {
    assert_int("0b1100 & 0b1010", 0b1000);
    assert_int("0b1100 | 0b1010", 0b1110);
    assert_int("0b1100 ~ 0b1010", 0b0110);
}

#[test]
fn bitwise_complement() {
    assert_int("~0", -1);
    assert_int("~5", -6);
    assert_int("~~5", 5);
    // `~` applies to the operand, not to the whole expression: `(~0) & 255`.
    assert_int("~0 & 0xFF", 255);
}

#[test]
fn shifts_fill_with_zeros() {
    assert_int("1 << 4", 16);
    assert_int("255 >> 4", 15);
    // `>>` is logical, as in Lua 5.3 — the sign bit is not replicated, so a
    // negative left operand comes back positive.
    assert_int("-1 >> 63", 1);
    assert_int("-1 >> 60", 15);
}

#[test]
fn a_negative_shift_count_shifts_the_other_way() {
    assert_int("1 << -1", 0);
    assert_int("16 >> -2", 64);
    assert_int("1 >> -4", 16);
}

#[test]
fn shifting_past_the_word_size_yields_zero() {
    // The case a bare Rust `<<` would panic on. Lua's rule: every bit
    // really has been shifted out.
    assert_int("1 << 64", 0);
    assert_int("1 << 999", 0);
    assert_int("-1 >> 64", 0);
    assert_int("1 >> 64", 0);
}

#[test]
fn bitwise_precedence_matches_the_parser() {
    assert_int("1 | 2 & 3", 3); // `1 | (2 & 3)`
    assert_int("1 ~ 3 & 1", 0); // `1 ~ (3 & 1)`
    assert_int("1 | 1 << 3", 9); // `1 | (1 << 3)`
    assert_int("1 << 2 + 1", 8); // `1 << (2 + 1)` — additive binds tighter
    assert_int("8 >> 1 >> 1", 2); // left-associative
}

#[test]
fn bitwise_comparison_needs_no_parentheses() {
    // Comparison is looser than every bitwise operator, which is what makes
    // the mask-test idiom read the way it looks.
    assert_bool("0b0110 & 0b0100 != 0", true);
    assert_bool("0b0110 & 0b0001 != 0", false);
}

#[test]
fn bitwise_compound_assignment() {
    assert_int("local n: integer = 0b0001\nn |= 0b0100\nn", 5);
    assert_int("local n: integer = 0b0101\nn &= 0b1100\nn", 4);
    assert_int("local n: integer = 1\nn <<= 5\nn", 32);
    assert_int("local n: integer = 32\nn >>= 4\nn", 2);
}

#[test]
fn bitwise_rejects_a_float_operand() {
    // The typechecker catches this on annotated code; the runtime is the
    // backstop for the unchecked `run()` entry point.
    assert!(matches!(
        eval("local f: float = 6.0\nf & 1").unwrap_err(),
        PipelineError::Typeck(_)
    ));
}

#[test]
fn bitwise_complement_rejects_a_float() {
    // Unary operand kinds are not a static check for any operator (`-s` on
    // a string is the same shape), so this one lands at runtime.
    //
    // The `;` is load-bearing. A newline is not a statement separator, so
    // `local f = 6.0` followed by a line starting with `~` would be read as
    // the *binary* `6.0 ~ f` — the same ambiguity `-` has had all along.
    assert!(matches!(
        eval("local f: float = 6.0;\n~f").unwrap_err(),
        PipelineError::Runtime(RuntimeError::TypeError { .. })
    ));
}

#[test]
fn a_leading_tilde_continues_the_previous_expression() {
    // Pinning the ambiguity above rather than leaving it to be rediscovered:
    // with no separator the `~` is xor, and with one it is complement.
    // No separator: the `~` was swallowed into the initializer, so `n` holds
    // `0b1100 ~ 0b1010`.
    assert_int("local n: integer = 0b1100\n~0b1010\nn", 0b0110);
    // With one, `~0b1010` is a statement of its own — the complement.
    assert_int("local n: integer = 0b1100;\n~0b1010", -11);
}

#[test]
fn bitwise_rejects_a_string_operand() {
    assert!(eval("local s: string = \"x\"\ns & 1").is_err());
}

mod bitwise_overloading {
    use super::*;

    /// A class overloading all five binary bitwise operators plus `~`.
    fn with_mask(expr: &str) -> String {
        format!(
            r#"
            class Mask implements OpBAnd, OpBOr, OpBXor, OpShl, OpShr, OpBNot
                local bits: integer
                fn init(b: integer)
                    self.bits = b
                end
                fn get() -> integer return self.bits end
                fn band(other: Mask) -> Mask return Mask(self.bits & other.get()) end
                fn bor(other: Mask) -> Mask return Mask(self.bits | other.get()) end
                fn bxor(other: Mask) -> Mask return Mask(self.bits ~ other.get()) end
                fn shl(other: Mask) -> Mask return Mask(self.bits << other.get()) end
                fn shr(other: Mask) -> Mask return Mask(self.bits >> other.get()) end
                fn bnot() -> Mask return Mask(~self.bits) end
            end
            {expr}
        "#
        )
    }

    #[test]
    fn each_operator_dispatches_to_its_contract_method() {
        assert_int(&with_mask("(Mask(0b1100) & Mask(0b1010)).get()"), 0b1000);
        assert_int(&with_mask("(Mask(0b1100) | Mask(0b1010)).get()"), 0b1110);
        assert_int(&with_mask("(Mask(0b1100) ~ Mask(0b1010)).get()"), 0b0110);
        assert_int(&with_mask("(Mask(1) << Mask(4)).get()"), 16);
        assert_int(&with_mask("(Mask(255) >> Mask(4)).get()"), 15);
        assert_int(&with_mask("(~Mask(0)).get()"), -1);
    }

    #[test]
    fn compound_assignment_uses_the_overload() {
        assert_int(
            &with_mask("local m: Mask = Mask(0b0001)\nm |= Mask(0b0100)\nm.get()"),
            0b0101,
        );
    }

    /// The other operand does not have to be the class itself — `shl` can
    /// declare any parameter type, which is what makes `mask << 4` (the
    /// natural shape for a shift) expressible rather than forcing a `Mask`
    /// to stand in for the shift count.
    #[test]
    fn an_overloads_other_operand_can_be_any_declared_type() {
        assert_int(
            r#"
            class Bits implements OpShl<integer, Bits>
                local bits: integer
                fn init(b: integer)
                    self.bits = b
                end
                fn get() -> integer return self.bits end
                fn shl(other: integer) -> Bits return Bits(self.bits << other) end
            end
            (Bits(1) << 4).get()
            "#,
            16,
        );
    }

    #[test]
    fn a_shift_dispatches_on_its_left_operand() {
        // Asymmetric like arithmetic: `2 << mask` must not quietly become
        // `mask << 2`.
        assert!(eval(&with_mask("2 << Mask(1)")).is_err());
    }
}

// ── OpIndex / OpNewIndex, Assignable<T> ───────────────────────────────────────

mod behaviour_contracts {
    use super::*;

    // ── OpIndex / OpNewIndex ──────────────────────────────────────────────

    fn with_cfg(expr: &str) -> String {
        format!(
            r#"
            class Cfg implements OpIndex<string, string>, OpNewIndex<string, string>
                local data: table<string, string>
                local reads: integer
                fn init()
                    self.data = {{}}
                    self.reads = 0
                end
                fn index(key: string) -> string
                    self.reads = self.reads + 1
                    return self.data[key] ?? "(unset)"
                end
                fn newIndex(key: string, value: string) -> nil
                    self.data[key] = String.lower(value)
                end
                fn reads() -> integer return self.reads end
            end
            {expr}
        "#
        )
    }

    #[test]
    fn index_and_new_index_dispatch_to_their_methods() {
        assert_str(
            &with_cfg("local c: Cfg = Cfg()\nc[\"h\"] = \"LOUD\"\nc[\"h\"]"),
            "loud",
        );
    }

    #[test]
    fn index_runs_on_every_read_not_only_on_a_miss() {
        // The difference from Lua's `__index`: an instance has no stored key
        // space, so the method *is* the lookup.
        assert_int(
            &with_cfg(
                "local c: Cfg = Cfg()\nc[\"a\"] = \"x\"\nlocal p: string = c[\"a\"]\n\
                 local q: string = c[\"a\"]\nc.reads()",
            ),
            2,
        );
    }

    #[test]
    fn a_key_that_was_never_written_still_answers() {
        assert_str(&with_cfg("Cfg()[\"nothing\"]"), "(unset)");
    }

    #[test]
    fn compound_assignment_runs_both_hooks() {
        // `c[k] ..= v` reads through `index` and writes through `newIndex`.
        assert_str(
            &with_cfg("local c: Cfg = Cfg()\nc[\"a\"] = \"AB\"\nc[\"a\"] ..= \"CD\"\nc[\"a\"]"),
            "abcd",
        );
    }

    #[test]
    fn a_hook_that_indexes_self_is_reported_rather_than_hanging() {
        assert!(
            eval(
                r#"
                class Loop implements OpIndex<string, string>
                    fn index(key: string) -> string return self[key] end
                end
                Loop()["x"]
                "#
            )
            .is_err()
        );
    }

    // ── Assignable ────────────────────────────────────────────────────────────

    /// A `Text` wrapping a `string`. Every method it exposes it declares
    /// itself, calling the `String` class explicitly — nothing is injected,
    /// and `string` has no members of its own.
    fn with_text(expr: &str) -> String {
        format!(
            r#"
            class Text implements Assignable<string>, OpToString
                local raw: string
                fn init(raw: string)
                    self.raw = raw
                end
                static fn of(s: string) -> Text return Text(s) end
                fn upper() -> string return String.upper(self.raw) end
                fn toString() -> string return self.raw end
            end
            {expr}
        "#
        )
    }

    #[test]
    fn assignable_builds_the_class_at_an_annotated_binding() {
        assert_str(&with_text("local t: Text = \"hello\"\nt.upper()"), "HELLO");
    }

    #[test]
    fn assignable_builds_the_class_at_a_parameter() {
        assert_str(
            &with_text(
                "fn take(t: Text) -> string return t.upper() end\nlocal out: string = take(\"hi\")\nout",
            ),
            "HI",
        );
    }

    #[test]
    fn assignable_leaves_a_value_of_the_target_class_alone() {
        // Already a `Text` — no second conversion, and no `from` call.
        assert_str(
            &with_text("local t: Text = Text(\"kept\")\nt.upper()"),
            "KEPT",
        );
    }

    #[test]
    fn assignable_leaves_nil_alone_in_a_nullable_slot() {
        assert_bool(&with_text("local t: Text? = nil\nt == nil"), true);
    }
}
