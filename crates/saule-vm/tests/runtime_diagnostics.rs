//! What a runtime error says: its message, the label under the caret, the
//! help line, and the line it points at.
//!
//! The bar is that both someone new to Saule and an experienced programmer
//! can fix the problem from the error alone. So each one is written in three
//! parts: the message names what went wrong precisely, in the standard terms
//! an experienced reader scans for; the label says in plain words what is
//! wrong with the underlined code; and the help says what to change — with
//! an example where one fits, and the advanced route as a short aside.
//! Every case below pins all three, because a regression in any of them is a
//! worse error that still "fails correctly".
//!
//! Each failing program sits beside a near-identical one that must run, so a
//! check that starts firing where it shouldn't fails here too. And no program
//! a user can write may produce an `internal error`: that wording is reserved
//! for bugs in the engine.

use miette::Diagnostic;
use saule_lexer::Lexer;
use saule_parser::parse;
use saule_runtime::RuntimeError;
use saule_vm::PipelineError;

/// Everything a reader sees of a runtime error.
#[derive(Debug, PartialEq)]
struct Shown {
    message: String,
    label: String,
    help: Option<String>,
    line: usize,
}

fn run(src: &str) -> Result<(), PipelineError> {
    let toks = Lexer::new(src).tokenize().expect("lex");
    let mut module = parse(toks).expect("parse");
    saule_vm::check_and_run(&mut module, "test.sau", src).map(|_| ())
}

fn runtime_error(src: &str) -> RuntimeError {
    let e = match run(src) {
        Err(PipelineError::Runtime(e)) => e,
        Err(other) => panic!("expected a runtime error, got: {other:?}\n{src}"),
        Ok(()) => panic!("expected a runtime error, but it ran:\n{src}"),
    };
    assert!(
        !matches!(e, RuntimeError::Internal { .. }),
        "a program reported an engine bug: {e}\n{src}"
    );
    e
}

/// Run `src`, which must fail at runtime, and return what it shows.
fn shown(src: &str) -> Shown {
    let e = runtime_error(src);
    let label = e
        .labels()
        .and_then(|mut l| l.next())
        .expect("every runtime error points somewhere");
    Shown {
        message: e.to_string(),
        label: label.label().unwrap_or_default().to_string(),
        help: e.help().map(|h| h.to_string()),
        line: src[..label.offset()].matches('\n').count() + 1,
    }
}

/// The source text the caret underlines.
fn pointed(src: &str) -> &str {
    let e = runtime_error(src);
    let label = e.labels().and_then(|mut l| l.next()).expect("a label");
    &src[label.offset()..label.offset() + label.len()]
}

fn ok(src: &str) {
    if let Err(e) = run(src) {
        panic!("expected this to run, got: {e:?}\n{src}");
    }
}

fn help(s: &str) -> Option<String> {
    Some(s.to_string())
}

// ── operators on values that arrived through `any` ─────────────────────────

#[test]
fn arithmetic_on_a_string() {
    assert_eq!(
        shown("local a: any = \"x\"\nlocal b: any = a * 2\n"),
        Shown {
            message: "cannot use `*` on a `string` and an `integer`".into(),
            label: "`*` needs two numbers here".into(),
            help: help(
                "convert the non-number first, e.g. `(s as integer) * 2` for a string holding a number; a class can define `*` by implementing `OpMul`"
            ),
            line: 2,
        }
    );
    ok("local a: any = 3\nlocal b: any = a * 2\n");
}

#[test]
fn comparing_a_string_with_a_number() {
    assert_eq!(
        shown("local a: any = \"x\"\nlocal b: any = 1\nlocal c: boolean = a < b\n"),
        Shown {
            message: "cannot compare a `string` with an `integer`".into(),
            label: "both sides must be the same kind of value".into(),
            help: help(
                "ordering compares two numbers or two strings; convert one side, e.g. `tostring(n)` or `(s as integer)`, or implement `OpCompare` on the class"
            ),
            line: 3,
        }
    );
    ok("local a: any = \"x\"\nlocal b: any = \"y\"\nlocal c: boolean = a < b\n");
}

#[test]
fn negating_a_string() {
    assert_eq!(
        shown("local s: any = \"x\"\nlocal n: any = -s\n"),
        Shown {
            message: "cannot negate a `string`".into(),
            label: "this is a `string`".into(),
            help: help(
                "`-` needs a number; check that this holds the value you expect, or implement `OpNeg` on the class"
            ),
            line: 2,
        }
    );
    ok("local s: any = 2\nlocal n: any = -s\n");
}

#[test]
fn complement_of_a_float_is_the_programs_mistake_not_an_engine_bug() {
    // The compiler used to emit the integer-only `BNOT` for any operand, so
    // a `float` reached it and was reported as a broken chunk.
    assert_eq!(
        shown("local f: float = 6.0;\nlocal x: any = ~f\n"),
        Shown {
            message: "cannot take the bitwise complement of a `float`".into(),
            label: "this is a `float`".into(),
            help: help(
                "`~` works on `integer`s only; convert with `as integer`, or implement `OpBNot` on the class"
            ),
            line: 2,
        }
    );
    // Through `any` as well.
    let s = shown("local f: any = 6.0;\nlocal x: any = ~f\n");
    assert_eq!(s.message, "cannot take the bitwise complement of a `float`");
    ok("local n: integer = 6;\nlocal x: integer = ~n\n");
    ok("local n: any = 6;\nlocal x: any = ~n\n");
}

#[test]
fn length_of_an_integer_uses_an() {
    let s = shown("local s: any = 5\nlocal n: integer = #s\n");
    assert_eq!(s.message, "cannot take the length of an `integer`");
    assert_eq!(s.label, "this is an `integer`");
    ok("local s: any = \"five\"\nlocal n: integer = #s\n");
}

#[test]
fn integer_to_a_negative_power() {
    assert_eq!(
        shown("local e: integer = -1\nlocal p: integer = 2 ^ e\n"),
        Shown {
            message: "negative exponent in integer `^`".into(),
            label: "the exponent here is -1".into(),
            help: help(
                "the result would be a fraction, which an `integer` can't hold; use floats: `(base as float) ^ -1.0`"
            ),
            line: 2,
        }
    );
    ok("local e: integer = 3\nlocal p: integer = 2 ^ e\n");
}

#[test]
fn integer_division_by_zero_but_not_float() {
    for op in ["/", "%"] {
        assert_eq!(
            shown(&format!(
                "local z: integer = 0\nlocal q: integer = 10 {op} z\n"
            )),
            Shown {
                message: "integer division by zero".into(),
                label: "the divisor here is 0".into(),
                help: help(
                    "check the divisor first: `if d != 0 then … end` (float division by zero gives `inf` instead)"
                ),
                line: 2,
            },
            "`{op}`"
        );
    }
    // IEEE 754: not an error.
    ok("local z: float = 0.0\nlocal q: float = 10.0 / z\n");
}

#[test]
fn a_zero_for_step() {
    let s = shown("local step: integer = 0\nfor i = 1, 3, step do end\n");
    assert_eq!(s.message, "`for` loop step cannot be 0");
    assert_eq!(s.label, "the step here is 0");
    assert_eq!(s.line, 2);
    assert!(s.help.is_some_and(|h| h.contains("`for i = 10, 1, -1 do`")));
    ok("local step: integer = -1\nfor i = 3, 1, step do end\n");
}

// ── calls ───────────────────────────────────────────────────────────────────

#[test]
fn calling_a_number() {
    assert_eq!(
        shown("local f: any = 5\nf()\n"),
        Shown {
            message: "cannot call an `integer`".into(),
            label: "this is an `integer`, not a function".into(),
            help: help(
                "only functions and classes can be called; check that this variable holds the function you expect"
            ),
            line: 2,
        }
    );
    ok("local f: any = fn() -> nil end\nf()\n");
}

#[test]
fn calling_nil_says_nil_without_an_article() {
    let s = shown("local f: any? = nil\nf(1)\n");
    assert_eq!(s.message, "cannot call `nil`");
    assert_eq!(s.label, "this is `nil`, not a function");
}

// ── members ─────────────────────────────────────────────────────────────────

const BOX: &str = "class Box\n  v: integer = 1\nend\n";

#[test]
fn reading_a_missing_member_names_the_fields_there_are() {
    assert_eq!(
        shown(&format!("{BOX}local b: any = Box()\nprintln(b.nope)\n")),
        Shown {
            message: "`Box` has no field or method `nope`".into(),
            label: "this is a `Box`".into(),
            help: help(
                "`Box` has the field `v`. Check the spelling (names are case-sensitive), or add the member to `class Box`"
            ),
            line: 5,
        }
    );
    ok(&format!("{BOX}local b: any = Box()\nprintln(b.v)\n"));
}

#[test]
fn several_fields_are_listed_in_the_plural() {
    let s = shown(
        "class P\n  x: integer = 1\n  y: integer = 2\nend\nlocal p: any = P()\nprintln(p.z)\n",
    );
    assert!(
        s.help
            .as_deref()
            .is_some_and(|h| h.starts_with("`P` has the fields `x`, `y`.")),
        "{:?}",
        s.help
    );
}

#[test]
fn assigning_a_missing_field() {
    assert_eq!(
        shown(&format!("{BOX}local b: any = Box()\nb.nope = 2\n")),
        Shown {
            message: "class `Box` has no field `nope`".into(),
            label: "assigned here".into(),
            help: help(
                "`Box` has the field `v`. Check the spelling, or declare `nope` in `class Box` — an object only has the fields its class declares"
            ),
            line: 5,
        }
    );
    ok(&format!("{BOX}local b: any = Box()\nb.v = 2\n"));
}

#[test]
fn calling_a_missing_method() {
    let s = shown(&format!("{BOX}local b: any = Box()\nb.nope()\n"));
    assert_eq!(s.message, "`Box` has no method `nope`");
    assert_eq!(s.label, "this is a `Box`");
    assert_eq!(s.line, 5);
    assert!(
        s.help
            .is_some_and(|h| h.contains("`Box` has the field `v`"))
    );
}

#[test]
fn a_class_starting_with_a_vowel_takes_an() {
    let s = shown("class Item\n  v: integer = 1\nend\nlocal b: any = Item()\nprintln(b.nope)\n");
    assert_eq!(s.label, "this is an `Item`");
}

#[test]
fn reading_a_member_of_a_number() {
    assert_eq!(
        shown("local n: any = 5\nprintln(n.x)\n"),
        Shown {
            message: "cannot read `.x` from an `integer`".into(),
            label: "this is an `integer`, which has no members".into(),
            help: help(
                "only objects, classes, enums and tables have members; check that this holds the value you expect"
            ),
            line: 2,
        }
    );
    // A table is the one kind where a missing key is just `nil`.
    ok("local t: any = {}\nprintln(t.x)\n");
}

// ── tables ──────────────────────────────────────────────────────────────────

#[test]
fn a_nil_table_key() {
    assert_eq!(
        shown("local t: table<any, integer> = {}\nlocal k: any? = nil\nt[k] = 1\n"),
        Shown {
            message: "cannot use `nil` as a table key".into(),
            label: "the key used here".into(),
            help: help(
                "a key must be an `integer`, a `string` or a `boolean`; check that the key has a value before using it"
            ),
            line: 3,
        }
    );
    ok("local t: table<any, integer> = {}\nlocal k: any = \"k\"\nt[k] = 1\n");
}

// ── `for … in` ──────────────────────────────────────────────────────────────

#[test]
fn looping_over_a_number() {
    assert_eq!(
        shown("local t: any = 5\nfor k, v in t do end\n"),
        Shown {
            message: "cannot iterate over an `integer`".into(),
            label: "this is an `integer`, not a collection".into(),
            help: help(
                "`for … in` takes a table, an iterator function, or an object with an `iter()` method (`Iterable`); to count, use `for i = 1, n do`"
            ),
            line: 2,
        }
    );
    ok("local t: any = {1, 2}\nfor k, v in t do end\n");
}

#[test]
fn a_loop_error_points_at_the_part_of_the_header_at_fault_not_the_whole_loop() {
    assert_eq!(
        pointed("local step: integer = 0\nfor i = 1, 3, step do\n  println(i)\nend\n"),
        "step"
    );
    assert_eq!(
        pointed("local t: any = 5\nfor k, v in t do\n  println(k)\nend\n"),
        "t"
    );
}

#[test]
fn an_operator_error_underlines_the_operation() {
    assert_eq!(
        pointed("local a: any = \"x\"\nlocal b: any = a * 2\n"),
        "a * 2"
    );
    assert_eq!(
        pointed("local z: integer = 0\nlocal q: integer = 10 / z\n"),
        "10 / z"
    );
}

// ── built-in functions ──────────────────────────────────────────────────────

#[test]
fn a_built_in_refusing_its_argument_is_named_in_the_label() {
    let s = shown("local o: string, n: integer = String.gsub(\"aaa\", \"a\", \"b\", -1)\n");
    assert_eq!(s.message, "String.gsub limit cannot be negative, got -1");
    assert_eq!(s.label, "`String.gsub` failed here");
    ok("local o: string, n: integer = String.gsub(\"aaa\", \"a\", \"b\", 1)\n");
}

#[test]
fn a_built_ins_own_name_is_not_repeated_in_the_message() {
    assert_eq!(
        shown("local t: table<integer, integer> = {1, 2}\nTable.insert(t, 9, 5)\n"),
        Shown {
            message: "position 5 is out of range for a table of length 2".into(),
            label: "`Table.insert` failed here".into(),
            help: help(
                "the position comes last, `Table.insert(t, value, pos)`, and must be from 1 to 3; leave it out to append\n\
                 signature: `Table.insert<V>(t: table<V>, value: V, pos: integer?) -> nil`"
            ),
            line: 2,
        }
    );
    // `Table.insert(t, value, pos)`: position 3 appends to a length-2 table.
    ok("local t: table<integer, integer> = {1, 2}\nTable.insert(t, 9, 3)\n");
}

// ── the standard library's own messages ────────────────────────────────────

#[test]
fn every_built_in_failure_ends_with_its_documented_signature() {
    for (src, signature) in [
        (
            "local o: string, n: integer = String.gsub(\"aaa\", \"a\", \"b\", -1)\n",
            "signature: `String.gsub(s: string, pat: string, repl: string, limit: integer?) -> (string, integer)`",
        ),
        (
            "local x: float = -9.0\nlocal r: float = Math.sqrt(x)\n",
            "signature: `Math.sqrt<N>(n: N) -> float`",
        ),
        (
            "local s: string = String.format(\"%s scored %d\", \"Ada\")\n",
            "signature: `String.format(fmt: string, ...any) -> string`",
        ),
    ] {
        let s = shown(src);
        assert!(
            s.help.as_deref().is_some_and(|h| h.ends_with(signature)),
            "{src}\n{:?}",
            s.help
        );
    }
}

#[test]
fn a_failed_assertion_has_no_signature_to_distract_from_its_message() {
    let s = shown("local x: integer? = nil\nassert(x, \"x must be set\")\n");
    assert_eq!(s.help, None);
}

#[test]
fn math_says_which_value_was_wrong_and_keeps_a_floats_point() {
    let s = shown("local x: float = -9.0\nlocal r: float = Math.sqrt(x)\n");
    assert_eq!(s.message, "square root of a negative number (-9.0)");
    assert!(
        s.help
            .is_some_and(|h| h.contains("`Math.sqrt(Math.abs(x))`"))
    );
    ok("local x: float = 9.0\nlocal r: float = Math.sqrt(x)\n");

    let s = shown("local c: integer = Math.clamp(5, 10, 1)\n");
    assert_eq!(
        s.message,
        "`Math.clamp` lower bound 10 is above its upper bound 1"
    );
    assert!(
        s.help
            .is_some_and(|h| h.starts_with("the order is `Math.clamp(n, lo, hi)`"))
    );
    ok("local c: integer = Math.clamp(5, 1, 10)\n");
}

#[test]
fn a_format_string_with_too_few_values_names_the_placeholder() {
    let s = shown("local s: string = String.format(\"%s scored %d\", \"Ada\")\n");
    assert_eq!(s.message, "placeholder 2 has no value to fill it");
    assert_eq!(s.label, "`String.format` failed here");
    ok("local s: string = String.format(\"%s scored %d\", \"Ada\", 3)\n");
}

#[test]
fn a_malformed_pattern_says_how_to_close_it() {
    let s = shown("local m: table<string>? = String.match(\"price: 10\", \"[0-9+\")\n");
    assert_eq!(s.message, "malformed pattern: a `[` set is never closed");
    assert!(s.help.is_some_and(|h| h.starts_with("end it with `]`")));
    ok("local m: table<string>? = String.match(\"price: 10\", \"[0-9]+\")\n");
}

#[test]
fn an_argument_message_names_the_argument_and_the_function() {
    // A comparator held as `any` is the one shape the checker lets through.
    let s = shown("local t: table<integer> = {3, 1}\nlocal cmp: any = 5\nTable.sort(t, cmp)\n");
    assert_eq!(
        s.message,
        "argument 2 of `Table.sort` must be a function, got an `integer`"
    );
    assert!(
        s.help
            .is_some_and(|h| h.starts_with("pass a comparator such as")),
        "the fix comes before the signature"
    );
    ok(
        "local t: table<integer> = {3, 1}\nlocal cmp: any = fn(a: integer, b: integer) -> boolean return a > b end\nTable.sort(t, cmp)\n",
    );
}

#[test]
fn a_failed_assertion() {
    let s = shown("local x: integer? = nil\nassert(x, \"x must be set\")\n");
    assert_eq!(s.message, "assertion failed: x must be set");
    assert_eq!(s.label, "this assertion failed");
    assert_eq!(s.line, 2);
    ok("local x: integer? = 1\nassert(x, \"x must be set\")\n");
}

// ── what every runtime error must avoid ────────────────────────────────────

const FAILING: &[&str] = &[
    "local a: any = \"x\"\nlocal b: any = a * 2\n",
    "local a: any = \"x\"\nlocal b: any = 1\nlocal c: boolean = a < b\n",
    "local f: any = 5\nf()\n",
    "local t: any = 5\nfor k, v in t do end\n",
    "local n: any = 5\nprintln(n.x)\n",
    "local z: integer = 0\nlocal q: integer = 10 / z\n",
    "local t: table<any, integer> = {}\nlocal k: any? = nil\nt[k] = 1\n",
    "local o: string, n: integer = String.gsub(\"aaa\", \"a\", \"b\", -1)\n",
];

#[test]
fn no_runtime_message_uses_the_old_catch_all() {
    for src in FAILING {
        let s = shown(src);
        assert!(!s.message.starts_with("type error"), "{}", s.message);
        assert_ne!(s.label, "type mismatch or invalid operation");
    }
}

#[test]
fn advice_goes_in_the_help_not_the_message() {
    // A message with ` — ` in it is a title with its advice run in.
    for src in FAILING {
        let s = shown(src);
        assert!(
            !s.message.contains(" — "),
            "advice in the message: {}",
            s.message
        );
    }
}

#[test]
fn no_runtime_message_uses_engine_jargon() {
    // Words that mean something to the implementer and nothing to someone
    // who just wrote the program.
    for src in FAILING {
        let s = shown(src);
        let text = format!("{} {} {}", s.message, s.label, s.help.unwrap_or_default());
        for word in [
            "dispatch", "operand", "vtable", "register", "opcode", "proto",
        ] {
            assert!(!text.contains(word), "`{word}` in: {text}");
        }
    }
}
