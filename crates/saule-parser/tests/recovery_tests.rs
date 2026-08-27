//! Error recovery: what survives in the tree when the input is wrong.
//!
//! Every test here asks the same question in a different shape — *the author
//! is mid-edit, what can the editor still answer?* — so the assertions are
//! about what the recovered tree **kept**, not about the diagnostics. The
//! diagnostics are checked too, but only to pin the two properties that make
//! them usable: at least one is reported, and a single mistake doesn't turn
//! into a page of them.

use saule_ast::*;
use saule_lexer::Lexer;
use saule_parser::{Parsed, PriorShape, parse, parse_recover, parse_recover_with_prior};

fn recover(src: &str) -> Parsed {
    let tokens = Lexer::new(src).tokenize().expect("lex ok");
    parse_recover(tokens, src)
}

/// The shape of `src`, which must parse cleanly — the "one keystroke ago"
/// version of the file under test.
fn shape_of(src: &str) -> PriorShape {
    let tokens = Lexer::new(src).tokenize().expect("lex ok");
    let module = parse(tokens).unwrap_or_else(|e| panic!("prior source must parse: {e:?}\n{src}"));
    PriorShape::of(&module)
}

/// Recover `src` knowing what it looked like when it last parsed cleanly.
fn recover_from(prior_src: &str, src: &str) -> Parsed {
    let prior = shape_of(prior_src);
    let tokens = Lexer::new(src).tokenize().expect("lex ok");
    parse_recover_with_prior(tokens, src, Some(&prior))
}

fn strict_fails(src: &str) {
    let tokens = Lexer::new(src).tokenize().expect("lex ok");
    assert!(
        parse(tokens).is_err(),
        "strict parse should still reject:\n{src}"
    );
}

/// Names of every `fn` declaration reachable in the tree, outermost first.
fn collect_fn_names<'a>(stmts: &'a [Spanned<Stmt>], out: &mut Vec<&'a str>) {
    for s in stmts {
        if let Stmt::Decl(d) = &s.value
            && let Decl::Function { name, body, .. } = &d.value
        {
            out.push(name);
            collect_fn_names(body, out);
        }
    }
}

/// Names of the top-level `fn` declarations in a module, in order.
fn fn_names(m: &Module) -> Vec<&str> {
    m.stmts
        .iter()
        .filter_map(|s| match &s.value {
            Stmt::Decl(d) => match &d.value {
                Decl::Function { name, .. } => Some(name.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

// ─── The invariant that makes two entry points safe ──────────────────────────

#[test]
fn strict_parse_is_unchanged_by_recovery() {
    // Recovery must never make invalid input parse. `parse` reports exactly
    // the inputs that produce holes.
    for src in [
        "local x = ",
        "fn f( end",
        "class C fn m() end",
        "if a then",
        "local x: = 1",
        "when(x)",
        "import from",
    ] {
        strict_fails(src);
    }
}

#[test]
fn valid_input_produces_no_errors_and_no_holes() {
    let src = r#"
        class Point
            x: integer = 0
            fn scale(self, k: integer) -> integer
                return self.x * k
            end
        end

        fn main() -> nil
            local p = Point()
            for i = 1, 10 do
                p.scale(i)
            end
            match p.x
                case 0 then "zero"
                case _ then "other"
            end
        end
    "#;
    let parsed = recover(src);
    assert!(parsed.is_ok(), "unexpected errors: {:?}", parsed.errors);
    assert!(!format!("{:?}", parsed.module).contains("Error"));
}

// ─── Layer 1: the closer that hasn't been typed yet ──────────────────────────

#[test]
fn function_being_typed_keeps_its_body() {
    // The single most common mid-edit state: the `end` isn't written yet.
    let parsed = recover("fn draw(w: integer) -> nil\n    local scale = w * 2\n");
    assert_eq!(parsed.errors.len(), 1);

    let Stmt::Decl(d) = &parsed.module.stmts[0].value else {
        panic!("expected a declaration, got {:?}", parsed.module.stmts[0]);
    };
    let Decl::Function {
        name, params, body, ..
    } = &d.value
    else {
        panic!("expected a function");
    };
    assert_eq!(name, "draw");
    assert_eq!(params.len(), 1, "the signature survives");
    assert_eq!(body.len(), 1, "and so does the body");
    assert!(matches!(&body[0].value, Stmt::Local { name, .. } if name == "scale"));
}

// ─── The forgotten `end` ─────────────────────────────────────────────────────
//
// Saule has no layout rule, so indentation is not evidence of anything —
// until an `end` goes missing, at which point it is the only evidence there
// is. `parse_recover` re-reads the file using it, but only after an ordinary
// parse has already reported a missing `end`, and only keeps the result if it
// doesn't strand an `end` that the plain reading consumed.

#[test]
fn missing_end_does_not_swallow_the_next_declaration() {
    // `after` is written at column 0, `before`'s body at column 4: it was
    // never meant to be nested, and the `end` between them is simply absent.
    let src = "fn before()\n    local a = 1\n\nfn after()\n    local b = 2\nend\n";
    let parsed = recover(src);
    assert_eq!(fn_names(&parsed.module), ["before", "after"]);

    // …and the declarations really are siblings, not one inside the other.
    let mut nested = Vec::new();
    collect_fn_names(&parsed.module.stmts, &mut nested);
    assert_eq!(nested, ["before", "after"]);
}

#[test]
fn a_run_of_forgotten_ends_is_untangled() {
    let src = "fn one()\n    local a = 1\n\nfn two()\n    local b = 2\n\nfn three()\n    local c = 3\nend\n";
    let parsed = recover(src);
    assert_eq!(fn_names(&parsed.module), ["one", "two", "three"]);
    // Two missing `end`s, reported as two mistakes rather than hidden inside
    // one enormous body — the repair raises the error count on purpose.
    assert_eq!(parsed.errors.len(), 2, "{:?}", parsed.errors);
}

#[test]
fn a_class_missing_its_end_does_not_adopt_the_next_declaration() {
    let src = "class Sprite\n    x: integer = 0\n\nfn helper()\n    local b = 2\nend\n";
    let parsed = recover(src);
    assert_eq!(fn_names(&parsed.module), ["helper"]);
    let Stmt::Decl(d) = &parsed.module.stmts[0].value else {
        panic!("expected a declaration");
    };
    let Decl::Class { name, members, .. } = &d.value else {
        panic!("expected a class, got {:?}", d.value);
    };
    assert_eq!(name, "Sprite");
    assert_eq!(members.len(), 1, "`helper` is not a method of Sprite");
}

#[test]
fn a_genuinely_nested_declaration_stays_nested() {
    // The rule must not fire on a file that closes its blocks properly, even
    // when the nested declaration is written well to the left of its siblings.
    let src = "fn outer()\n    local x = 1\n    fn nested()\n    end\nend\n";
    let parsed = recover(src);
    assert!(parsed.is_ok(), "{:?}", parsed.errors);
    assert_eq!(fn_names(&parsed.module), ["outer"]);
}

#[test]
fn a_dedent_that_would_strand_an_end_is_not_trusted() {
    // Sloppy indentation *and* a real missing `end` further down. Reading
    // `fn inner` as a dedent would close `outer` early and leave its `end`
    // closing nothing — so the plain reading wins and `inner` stays nested.
    let src = "fn outer()\n        local x = 1\n    fn inner()\n    end\nend\nfn other()\n";
    let parsed = recover(src);
    assert_eq!(fn_names(&parsed.module), ["outer", "other"]);
    let mut nested = Vec::new();
    collect_fn_names(&parsed.module.stmts, &mut nested);
    assert_eq!(nested, ["outer", "inner", "other"]);
}

#[test]
fn an_unindented_file_falls_back_to_nesting_without_history() {
    // No indentation means no evidence *in the file*, so the offside rule
    // cannot fire on its own. The declaration is still in the tree — just one
    // level deeper than intended. `recovered_using_history` below is what
    // fixes this when the editor has seen the file in a valid state.
    let src = "fn before()\nlocal a = 1\nfn after()\nlocal b = 2\nend\n";
    let parsed = recover(src);
    let mut nested = Vec::new();
    collect_fn_names(&parsed.module.stmts, &mut nested);
    assert_eq!(nested, ["before", "after"]);
}

// ─── …and what the last clean parse remembers ────────────────────────────────
//
// Indentation is evidence inside the file. History is evidence about the file:
// a declaration that has sunk into a block it was never in was sunk by a
// deleted `end`, whatever the whitespace says. These are the three shapes
// indentation cannot see.

#[test]
fn history_untangles_an_unindented_file() {
    let before = "fn before()\nlocal a = 1\nend\n\nfn after()\nlocal b = 2\nend\n";
    let now = "fn before()\nlocal a = 1\n\nfn after()\nlocal b = 2\nend\n";
    assert_eq!(
        fn_names(&recover_from(before, now).module),
        ["before", "after"]
    );
}

#[test]
fn history_untangles_an_empty_body() {
    // Nothing in the block, so there is no body column to be left of.
    let before = "fn before()\nend\n\nfn after()\nlocal b = 2\nend\n";
    let now = "fn before()\n\nfn after()\nlocal b = 2\nend\n";
    assert_eq!(
        fn_names(&recover_from(before, now).module),
        ["before", "after"]
    );
}

#[test]
fn history_untangles_a_declaration_at_the_body_column() {
    // Indented to exactly the body's column, so the offside rule reads it as
    // belonging there.
    let before = "fn before()\n    local a = 1\nend\n\nfn after()\n    local b = 2\nend\n";
    let now = "fn before()\n    local a = 1\n\n    fn after()\n    local b = 2\nend\n";
    assert_eq!(
        fn_names(&recover_from(before, now).module),
        ["before", "after"]
    );
}

#[test]
fn history_untangles_an_unindented_class() {
    let before = "class Sprite\nx: integer = 0\nend\n\nfn helper()\nlocal b = 2\nend\n";
    let now = "class Sprite\nx: integer = 0\n\nfn helper()\nlocal b = 2\nend\n";
    let parsed = recover_from(before, now);
    assert_eq!(fn_names(&parsed.module), ["helper"]);
}

#[test]
fn history_keeps_a_genuinely_nested_declaration_nested() {
    // `nested` lived at depth 1 before and is at depth 1 now, so nothing has
    // sunk and the rule stays quiet — even though the file is unindented and
    // `outer`'s `end` really is missing.
    let before = "fn outer()\nlocal x = 1\nfn nested()\nend\nend\n";
    let now = "fn outer()\nlocal x = 1\nfn nested()\nend\n";
    let parsed = recover_from(before, now);
    let mut nested = Vec::new();
    collect_fn_names(&parsed.module.stmts, &mut nested);
    assert_eq!(nested, ["outer", "nested"]);
}

#[test]
fn an_empty_or_absent_history_changes_nothing() {
    // The shape is one of two pieces of evidence, never a requirement.
    let src = "fn before()\n    local a = 1\n\nfn after()\n    local b = 2\nend\n";
    let tokens = Lexer::new(src).tokenize().expect("lex");
    let with_empty = parse_recover_with_prior(tokens, src, Some(&PriorShape::default()));
    assert_eq!(fn_names(&with_empty.module), ["before", "after"]);
    assert_eq!(fn_names(&recover(src).module), ["before", "after"]);
}

#[test]
fn a_stale_history_cannot_break_valid_code() {
    // The shape describes a file that no longer exists — `after` has since
    // been made a deliberate inner function. A clean parse never consults it.
    let stale = "fn before()\nend\n\nfn after()\nend\n";
    let now = "fn before()\n    local a = 1\n    fn after()\n    end\nend\n";
    let parsed = recover_from(stale, now);
    assert!(parsed.is_ok(), "{:?}", parsed.errors);
    let mut nested = Vec::new();
    collect_fn_names(&parsed.module.stmts, &mut nested);
    assert_eq!(nested, ["before", "after"]);
}

#[test]
fn an_unnameable_declaration_does_not_swallow_the_file() {
    // The other half: when the parser can't identify the `fn` at all it must
    // not open a body for it, or the good declarations after it are gone.
    let parsed = recover("fn = = =\n\nfn good()\n    local a = 1\nend\n");
    assert_eq!(fn_names(&parsed.module), ["good"]);
}

#[test]
fn signature_being_typed_keeps_the_declaration() {
    // `fn` and the name typed, the `(` not yet.
    let parsed = recover("fn render\n    local n = 1\nend\n");
    assert_eq!(fn_names(&parsed.module), ["render"]);
}

#[test]
fn unclosed_call_still_produces_a_call_node() {
    // What signature help needs: the callee resolved and the arguments so far.
    let parsed = recover("draw(10, 20");
    let Stmt::Expr(e) = &parsed.module.stmts[0].value else {
        panic!("expected an expression statement");
    };
    let Expr::Call { callee, args, .. } = &e.value else {
        panic!("expected a call, got {:?}", e.value);
    };
    assert!(matches!(&callee.value, Expr::Ident(n) if n == "draw"));
    assert_eq!(args.len(), 2);
}

// ─── Layer 2: holes where an operand was required ────────────────────────────

#[test]
fn local_without_a_value_still_binds_its_name() {
    // The reason holes exist rather than dropping the statement: `pos` has to
    // stay in scope for the completion request on the line below.
    let parsed = recover("local pos =\nlocal size = 2\n");
    assert_eq!(parsed.module.stmts.len(), 2);
    match &parsed.module.stmts[0].value {
        Stmt::Local { name, value, .. } => {
            assert_eq!(name, "pos");
            assert!(matches!(value, Some(v) if v.value == Expr::Error));
        }
        other => panic!("expected `local pos`, got {other:?}"),
    }
    assert!(matches!(&parsed.module.stmts[1].value, Stmt::Local { name, .. } if name == "size"));
}

#[test]
fn missing_type_annotation_becomes_any() {
    let parsed = recover("local n: = 1");
    match &parsed.module.stmts[0].value {
        Stmt::Local { name, ty, .. } => {
            assert_eq!(name, "n");
            assert_eq!(ty.as_ref(), Some(&Type::Named("any".into())));
        }
        other => panic!("expected a local, got {other:?}"),
    }
}

#[test]
fn member_access_with_nothing_after_the_dot() {
    // What completion sees the instant `.` is typed.
    let parsed = recover("local c = player.\nlocal after = 1\n");
    assert_eq!(parsed.module.stmts.len(), 2, "the next line still parses");
    let Stmt::Local { value: Some(v), .. } = &parsed.module.stmts[0].value else {
        panic!("expected an initialized local");
    };
    match &v.value {
        Expr::Member { obj, name } => {
            assert!(matches!(&obj.value, Expr::Ident(n) if n == "player"));
            assert_eq!(name, "", "the placeholder name is empty");
        }
        other => panic!("expected a member access, got {other:?}"),
    }
}

// ─── Layer 3: resynchronisation ──────────────────────────────────────────────

#[test]
fn a_broken_statement_costs_only_that_statement() {
    let parsed = recover("local a = 1\n) ] } =\nlocal b = 2\n");
    let kinds: Vec<&str> = parsed
        .module
        .stmts
        .iter()
        .map(|s| match &s.value {
            Stmt::Local { name, .. } => name.as_str(),
            Stmt::Error => "<error>",
            _ => "<other>",
        })
        .collect();
    assert!(
        kinds.contains(&"a") && kinds.contains(&"b"),
        "got {kinds:?}"
    );
}

#[test]
fn a_broken_class_member_costs_only_that_member() {
    let src = r#"
        class Sprite
            x: integer = 0
            fn = = =
            fn update(self) -> nil
                self.x = self.x + 1
            end
            y: integer = 0
        end
    "#;
    let parsed = recover(src);
    let Stmt::Decl(d) = &parsed.module.stmts[0].value else {
        panic!("expected a declaration");
    };
    let Decl::Class { name, members, .. } = &d.value else {
        panic!("expected a class, got {:?}", d.value);
    };
    assert_eq!(name, "Sprite");

    let mut fields = Vec::new();
    let mut methods = Vec::new();
    for m in members {
        match &m.value {
            ClassMember::Field { name, .. } => fields.push(name.as_str()),
            ClassMember::Method(m) => methods.push(m.name.as_str()),
        }
    }
    assert_eq!(fields, ["x", "y"], "fields on both sides survive");
    assert!(methods.contains(&"update"), "so does the good method");
}

#[test]
fn a_broken_statement_inside_a_function_keeps_the_function() {
    let src = "fn f()\n    local a = 1\n    ) ] }\n    local b = 2\nend\n\nfn g()\nend\n";
    let parsed = recover(src);
    assert_eq!(fn_names(&parsed.module), ["f", "g"]);
}

// ─── Diagnostics stay usable ─────────────────────────────────────────────────

#[test]
fn one_mistake_does_not_become_a_page_of_errors() {
    // Every rule that fails after the first one points at the same token;
    // only the first is worth showing.
    let parsed = recover("local x = ");
    assert_eq!(parsed.errors.len(), 1, "{:?}", parsed.errors);
}

#[test]
fn errors_are_ordered_and_bounded() {
    let src = "= ".repeat(500);
    let parsed = recover(&src);
    assert!(!parsed.errors.is_empty());
    assert!(
        parsed.errors.len() <= saule_parser::MAX_ERRORS,
        "got {} errors",
        parsed.errors.len()
    );
    assert!(
        parsed
            .errors
            .windows(2)
            .all(|w| w[0].span().start < w[1].span().start),
        "errors should be in strictly increasing source order"
    );
}

#[test]
fn several_independent_mistakes_are_all_reported() {
    // The point of not bailing: the error on line 5 is visible while the one
    // on line 1 is still there.
    let parsed = recover("local a =\nlocal b = 2\nlocal c: = 3\nlocal d =\n");
    assert!(
        parsed.errors.len() >= 3,
        "expected one per mistake, got {:?}",
        parsed.errors
    );
}

// ─── Termination ─────────────────────────────────────────────────────────────

#[test]
fn pathological_input_terminates() {
    // Recovery's failure mode is an infinite loop: a rule that reports an
    // error without consuming a token, inside a loop that doesn't force
    // progress. These are the shapes that would trip it.
    for src in [
        "then",
        "end end end",
        ", , ,",
        "= = =",
        "((((",
        "))))",
        "{{{{",
        "local",
        "local local local",
        "fn",
        "class",
        "class class",
        "import",
        "match",
        "case then",
        "when(",
        "do",
        "for",
        "for in do",
        "if then else end",
        "repeat until",
        "try catch",
        "->",
        "...",
        "a.",
        "a?.",
        "a[",
        "a(",
        "interface fn",
        "enum",
        "export",
        "local x: table<",
        "f<T",
    ] {
        // The assertion is that this returns at all.
        let parsed = recover(src);
        assert!(
            parsed.errors.len() <= saule_parser::MAX_ERRORS,
            "{src:?} exceeded the error cap"
        );
    }
}

#[test]
fn deeply_nested_junk_still_terminates() {
    // Run on a big stack, like the real binaries do: recursive descent turns
    // nesting into native recursion, and `MAX_NESTING_DEPTH` is set against
    // the 1 GiB stack `saule` and `saule-lsp` give the parser, not against a
    // test harness thread's default couple of megabytes.
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(|| {
            let parsed = recover(&"(".repeat(5_000));
            assert!(!parsed.errors.is_empty());
        })
        .expect("spawn")
        .join()
        .expect("the parser should return rather than recurse forever");
}

// ─── Recovery must not change what valid code means ──────────────────────────

#[test]
fn speculative_parses_are_not_recovered_into_success() {
    // `(1 + 2)` is a parenthesised expression. A probe for an arrow-lambda
    // header that recovered from "expected parameter name" would read it as a
    // lambda instead — so speculation has to run with recovery off.
    let parsed = recover("local x = (1 + 2) * 3");
    assert!(parsed.is_ok(), "{:?}", parsed.errors);
    let Stmt::Local { value: Some(v), .. } = &parsed.module.stmts[0].value else {
        panic!("expected an initialized local");
    };
    assert!(
        matches!(&v.value, Expr::Binary { op: BinOp::Mul, .. }),
        "got {:?}",
        v.value
    );
}

#[test]
fn less_than_is_still_less_than() {
    // The other probe: `a < b` must not be read as a generic instantiation
    // just because a recovered `parse_type` would accept anything.
    let parsed = recover("local ok = a < b and c > d");
    assert!(parsed.is_ok(), "{:?}", parsed.errors);
}

#[test]
fn return_before_elseif_parses() {
    // Fixed in passing: `elseif` can only follow a completed block, so it
    // ends `return`'s value list rather than starting it.
    let parsed = recover("if a then\n    return\nelseif b then\n    return 1\nend\n");
    assert!(parsed.is_ok(), "{:?}", parsed.errors);
}
