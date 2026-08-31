//! Module-body ordering, the proto pre-pass, and forward references (§14).

use crate::harness::*;
use saule_lexer::Lexer;
use saule_parser::parse;

// ── module-body straight-line order vs. the proto pre-pass ────────────────

/// Parse and analyse without asserting the module is clean.
///
/// `front_end` refuses to hand back a module carrying semantic errors,
/// which is the right default. The forward-reference programs below are
/// ones the *tree-walker* rejects at run time rather than at analysis, so
/// they analyse clean and still need to reach the compiler.
fn front_end_unchecked(src: &str) -> saule_ast::Module {
    saule_interpreter::init();
    let toks = Lexer::new(src).tokenize().expect("lex");
    let module = parse(toks).expect("parse");
    let _ = saule_interpreter::analyze_and_prepare(&module, saule_semantic::ModuleSeed::default());
    module
}

#[test]
fn a_module_level_forward_call_is_refused_not_miscompiled() {
    // The divergence this guard exists for. `fn_protos` is pre-collected so
    // a forward call resolves, which is correct *inside* a function body —
    // one cannot run before the module body finishes — and wrong for the
    // module body, which runs top to bottom. The tree-walker finds `later`
    // still undefined and errors; the VM used to answer 105 from the proto
    // table. Right exit status, invented value.
    //
    // Refusing hands the module to the tree-walker, which is what *defines*
    // the behaviour. Reproducing its diagnostic instead would mean keeping
    // two error strings in step forever; falling back makes them agree by
    // construction.
    let src = "local r = later(5)\n\
               println(tostring(r))\n\
               fn later(x: integer) -> integer\n\
               \x20 return x + 100\n\
               end";
    let module = front_end_unchecked(src);
    match saule_vm::compile(&module, "x.sau", src) {
        Err(saule_vm::CompileError::Unsupported { span, .. }) => {
            assert!(span.start < span.end, "the refusal must point somewhere");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn a_forward_reference_reached_through_a_callee_is_refused() {
    // `C.go()` above `fn later`, where `go`'s body reads `later`. The
    // reference inside `go` is legal — a forward reference in a function
    // body is ordinary Saule — and only the *call* is early, so nothing at
    // the call site distinguishes a callee that reaches an undeclared name
    // from one that does not.
    //
    // This used to diverge: the tree-walker errored, the VM resolved the
    // proto and returned 101. It was pinned here as *expected* divergence,
    // with a note that closing it needed call-graph reachability. That is
    // now what `Compiler::reaches_undeclared` does — one edge per mention,
    // closed transitively at the call site.
    //
    // A blunter guard was tried before this and reverted: "refuse any
    // module-body call while a `fn` is still ahead" refused
    // `a_module_level_parallel_local_writes_module_slots` and
    // `the_recursion_guard_still_unwinds_after_re_entrant_calls`, because a
    // call partway down a file with any `fn` below it is an ordinary shape.
    // Those two still pass, which is half of what this test is worth.
    let src = "class C\n\
               \x20 static fn go() -> integer\n\
               \x20   return later(1)\n\
               \x20 end\n\
               end\n\
               local r: integer = C.go()\n\
               fn later(x: integer) -> integer\n\
               \x20 return x + 100\n\
               end\nr";
    let module = front_end_unchecked(src);
    match saule_vm::compile(&module, "diff.sau", src) {
        Err(saule_vm::CompileError::Unsupported { thing, .. }) => assert_eq!(
            thing,
            "a module-level call whose callee reaches a declaration further down"
        ),
        other => panic!(
            "expected a refusal so the tree-walker defines the diagnostic, got {other:?}"
        ),
    }
    // And the fallback really does reproduce the oracle: the tree-walker is
    // what reports it, so the two agree by construction.
    assert!(matches!(tree_walker(&module), Outcome::Error(_)));
}

#[test]
fn a_module_level_call_below_the_declaration_still_compiles() {
    // The over-refusal guard. The fix above is a *positional* test, and the
    // failure mode of getting it wrong is silent: every ordinary top-level
    // call would fall back and nothing would fail, only slow down.
    must_agree(
        "fn helper(x: integer) -> integer
           return x + 1
         end
         local a: integer = helper(1)
         a",
    );
}

#[test]
fn a_forward_call_inside_a_function_body_still_compiles() {
    // `fn a() b() end` above `fn b()` is ordinary Saule and must stay a
    // `CALLK` — the whole reason `fn_protos` is pre-collected. The guard
    // must not reach inside a function body, where the pre-pass is right.
    must_agree(
        "fn a(x: integer) -> integer
           return b(x) + 1
         end
         fn b(x: integer) -> integer
           return x * 2
         end
         local r: integer = a(5)
         r",
    );
}


#[test]
fn deep_recursion_hits_the_same_limit_under_both_engines() {
    // A limit is observable behaviour, not an implementation detail.
    //
    // §6.4 argues the VM's frame cap can be two orders of magnitude above
    // the tree-walker's, and the argument is sound in isolation: a call is a
    // `Vec` push here, not a native stack frame, so `MAX_EVAL_DEPTH`'s
    // reason for existing (a SIGSEGV that cannot be caught) does not apply.
    // Set to 1_000_000 on that reasoning, it made `depth(50_000)` return
    // `50000` under `--vm` and raise `StackOverflow` without it.
    //
    // While the tree-walker is the default and the VM is an opt-in
    // accelerator behind a silent fallback, "works with `--vm`, crashes
    // without it" is exactly what that fallback exists to prevent. The
    // raise belongs with Phase 4, where flipping the default makes it an
    // announced improvement rather than a disagreement.
    //
    // Asserted on the constants rather than by actually recursing: the
    // tree-walker needs ~10_000 *native* frames to reach its limit, which
    // is more stack than libtest's test thread has — the same reason
    // `tests/ui/stack_overflow_reentrant.sau` runs `saule` as a process.
    assert_eq!(
        saule_vm::vm::DEFAULT_MAX_FRAMES,
        saule_interpreter::eval::MAX_EVAL_DEPTH as usize,
        "the VM's frame cap drifted from the tree-walker's depth limit — \
         the two engines would then disagree about how deep is too deep"
    );
}


// ── forward references through a callee ───────────────────────────────────

#[test]
fn a_module_body_call_whose_callee_is_fully_declared_still_compiles() {
    // The other half of the reachability guard: it must not refuse an
    // ordinary program. `helper` is declared above the call and reaches
    // only names above it, so nothing here is early — and a `fn` declared
    // *below* the call that the callee never touches must not matter.
    let src = "fn helper(x: integer) -> integer\n  return x * 2\nend\n\
               local r: integer = helper(21)\n\
               fn unrelated(y: integer) -> integer\n  return y\nend\nr";
    must_agree(src);
    assert!(
        !disasm_of(src).is_empty(),
        "the reachability guard must not refuse a program with an unrelated \
         `fn` below the call"
    );
}

#[test]
fn a_module_body_call_reaching_a_later_class_is_refused() {
    // The same shape one level down, through a class rather than a `fn`:
    // `run()` constructs `Later`, which is declared below the call.
    let src = "fn run() -> integer\n  local l: Later = Later()\n  return l.v\nend\n\
               local r: integer = run()\n\
               class Later\n\
               \x20 v: integer\n\
               \x20 fn init()\n    self.v = 5\n  end\n\
               end\nr";
    let module = front_end_unchecked(src);
    match saule_vm::compile(&module, "diff.sau", src) {
        Err(saule_vm::CompileError::Unsupported { thing, .. }) => assert_eq!(
            thing,
            "a module-level call whose callee reaches a declaration further down"
        ),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn a_literal_default_skipped_in_the_middle_is_filled() {
    // The case the entry stubs structurally cannot serve: `pad` is skipped
    // while `body`, which comes *after* it, is supplied. Stubs fill a
    // suffix, so there is no entry point meaning "fill slot 1 but not
    // slot 2", and this refused until the literal default was materialized
    // at the call site.
    //
    // Both call shapes are asserted, because they take different paths: the
    // one that skips the middle default and the one that supplies it.
    must_agree(
        "fn box(title: string, pad: integer = 7, tail: string) -> string\n\
         \x20 return title .. \":\" .. tostring(pad) .. \":\" .. tail\n\
         end\n\
         local a: string = box(title: \"t\", tail: \"z\")\n\
         local b: string = box(title: \"t\", pad: 1, tail: \"z\")\n\
         a .. \"/\" .. b",
    );
}

#[test]
fn every_scalar_literal_default_can_be_skipped_in_the_middle() {
    // One per literal shape the call site is allowed to materialize, so a
    // future edit that narrows the set fails here rather than silently
    // sending these back to the tree-walker.
    must_agree(
        "fn f(a: integer, i: integer = -4, s: string = \"d\", b: boolean = true, x: float = 2.5, t: string) -> string\n\
         \x20 return tostring(a) .. tostring(i) .. s .. tostring(b) .. tostring(x) .. t\n\
         end\n\
         f(a: 1, t: \"end\")",
    );
}

#[test]
fn a_skipped_middle_default_still_runs_in_the_callee_when_it_is_not_a_literal() {
    // The other half of the rule, and the reason the literal restriction is
    // not merely conservative. A default that reads an earlier *parameter*
    // cannot be materialized at the call site — `a` there is the caller's
    // `a`, not the callee's.
    //
    // This refused outright until the gap mask: the call skips `d` while
    // supplying `t` after it, which no per-arity entry stub can express. Now
    // the caller passes a bitmask naming `d`, and the callee's gap entry runs
    // `a * 2` in its own frame.
    //
    // The literal value is asserted, not just agreement: `6` is the callee's
    // `a` doubled, and `200` is what a compiler that materialized the default
    // at the call site would have produced from the caller's `a`. Two engines
    // agreeing on `200` would be two engines that are both wrong.
    let src = "local a: integer = 100\n\
               fn f(a: integer, d: integer = a * 2, t: string) -> string\n\
               \x20 return tostring(d) .. t\n\
               end\n\
               f(a: 3, t: \"!\")";
    must_agree(src);
    assert_eq!(
        tree_walker(&front_end(src)),
        Outcome::Value("string:6!".into())
    );
}


#[test]
fn an_exported_module_variable_is_read_and_written() {
    // `export name: T = value` is the module-scope counterpart of a class
    // field, and the resolver already gives it a module slot -- the compiler
    // simply had no branch for `Decl::Variable` and refused the whole module
    // as `a declaration the compiler does not handle`.
    must_agree(
        "export appName: string = \"Saule\"\n         export version: integer = 26\n         export pending: string?\n         version = version + 1\n         appName .. \" v\" .. version .. \" \" .. (pending ?? \"none\")",
    );
}


// ── §19 the gap mask ──────────────────────────────────────────────────────
//
// A call that skips a defaulted parameter while supplying one after it. The
// per-arity entry stubs fill a *suffix*, so the caller names the skipped
// slots in a bitmask and the callee's gap entry runs those defaults — in the
// callee's own frame, in parameter order.

#[test]
fn a_skipped_default_with_a_side_effect_runs_once_in_the_callee() {
    // *When* the default runs is observable, not just what it evaluates to.
    // The tree-walker evaluates it during binding — after every argument,
    // before the body — and exactly once. `log` records the order.
    must_agree(
        "local log: string = \"\"\n\
         fn note(s: string) -> string\n\
         \x20 log = log .. s\n\
         \x20 return s\n\
         end\n\
         fn f(a: string, d: string = note(\"D\"), t: string) -> string\n\
         \x20 return a .. d .. t\n\
         end\n\
         local r = f(a: note(\"A\"), t: note(\"T\"))\n\
         r .. \"/\" .. log",
    );
}

/// A module-level `a` and `b`, shadowed by the parameters of every callee
/// below.
///
/// Not decoration. Saule's resolver only accepts a default that mentions an
/// earlier *parameter* when that name also exists at module scope — so this
/// prefix is what makes the interesting cases legal source at all, and it is
/// simultaneously the trap: `100` and `200` are what a call site that
/// materialized these defaults itself would bind, and the callee's own
/// parameters are what it must bind instead.
const SHADOWED: &str = "local a: integer = 100\nlocal b: integer = 200\n";

#[test]
fn two_skipped_defaults_are_filled_in_parameter_order() {
    // The second default reads the first, so filling them out of order — or
    // filling only one — is visible in the answer.
    let src = format!(
        "{SHADOWED}\
         fn f(a: integer, b: integer = a + 1, c: integer = b + 1, t: string) -> string\n\
         \x20 return tostring(a) .. tostring(b) .. tostring(c) .. t\n\
         end\n\
         f(a: 1, t: \"!\")"
    );
    must_agree(&src);
    // Spelled out: `123!` is the chain resolved against the callee's own
    // parameters. `1201!` would be `b` from module scope.
    assert_eq!(
        tree_walker(&front_end(&src)),
        Outcome::Value("string:123!".into())
    );
}

#[test]
fn a_gap_and_a_trailing_default_are_both_filled() {
    // Once a mask is needed the call passes the *whole* parameter list, which
    // pulls the trailing defaults in with it. They need mask bits of their
    // own — computing the mask only over the middle would leave `e` nil.
    must_agree(&format!(
        "{SHADOWED}\
         fn f(a: integer, b: integer = a + 1, t: string, e: integer = a + 100) -> string\n\
         \x20 return tostring(a) .. tostring(b) .. t .. tostring(e)\n\
         end\n\
         f(a: 2, t: \"-\")"
    ));
}

#[test]
fn a_literal_and_a_non_literal_default_can_be_skipped_together() {
    // The literal is still materialized at the call site (no bit), the other
    // is left to the callee (bit set). Both routes in one call.
    must_agree(&format!(
        "{SHADOWED}\
         fn f(a: integer, lit: integer = 9, other: integer = a * 3, t: string) -> string\n\
         \x20 return tostring(a) .. tostring(lit) .. tostring(other) .. t\n\
         end\n\
         f(a: 4, t: \"!\")"
    ));
}

#[test]
fn an_explicit_nil_is_a_value_and_does_not_trigger_the_default() {
    // Why the mask exists at all, rather than the callee testing each slot
    // for `nil`. `bind_params` treats a supplied `nil` as a value: `d` stays
    // nil here even though it has a default. A nil-testing gap entry would
    // hand back 99 — and `e`, genuinely absent, must still get its default,
    // so the two cases have to be told apart within one call.
    must_agree(
        "fn f(a: integer, d: integer? = 99, e: integer? = 77, t: string) -> string\n\
         \x20 return tostring(a) .. (d == nil and \"nil\" or tostring(d)) .. tostring(e) .. t\n\
         end\n\
         f(a: 1, d: nil, t: \"!\")",
    );
}

#[test]
fn a_skipped_default_reads_the_callees_module_scope() {
    // The other thing a default may reach that the call site cannot: a name
    // in the callee's module. Same file here, so `SCALE` happens to be
    // visible either way — the cross-module form is `examples/UI Project`,
    // where the default is an enum variant the calling module never imported
    // and materializing it at the call site could not have resolved at all.
    must_agree(
        "local SCALE: integer = 10\n\
         fn f(d: integer = SCALE * 3, t: string) -> string\n\
         \x20 return tostring(d) .. t\n\
         end\n\
         f(t: \"!\")",
    );
}

#[test]
fn a_constructor_fills_a_skipped_default() {
    // `VStack(alignment: …, sizing: …, children: …)`, reduced. A constructor
    // enters through `CALLM` with the receiver at argument 0, so its gap
    // entry sits one arity higher than a plain function's — the arithmetic
    // that indexes it has to account for `self`.
    let src = format!(
        "{SHADOWED}local TAG: string = \"T\"\n\
         class Box\n\
         \x20 a: integer\n\
         \x20 m: string\n\
         \x20 t: string\n\
         \x20 fn init(a: integer, m: string = TAG .. tostring(a), t: string = \"z\")\n\
         \x20   self.a = a\n\
         \x20   self.m = m\n\
         \x20   self.t = t\n\
         \x20 end\n\
         end\n\
         local x = Box(a: 5, t: \"!\")\n\
         tostring(x.a) .. x.m .. x.t"
    );
    must_agree(&src);
    // `T5`, from the constructor's own `a` — not `T100` from module scope.
    assert_eq!(
        tree_walker(&front_end(&src)),
        Outcome::Value("string:5T5!".into())
    );
}

#[test]
fn a_method_fills_a_skipped_default() {
    // A default that reaches `self`: only the callee has one.
    must_agree(&format!(
        "{SHADOWED}\
         class Box\n\
         \x20 n: integer\n\
         \x20 fn init(n: integer)\n    self.n = n\n  end\n\
         \x20 fn go(a: integer, d: integer = a + self.n, t: string) -> string\n\
         \x20   return tostring(d) .. t\n\
         \x20 end\n\
         end\n\
         local x = Box(10)\n\
         x.go(a: 5, t: \"!\")"
    ));
}

#[test]
fn a_static_method_fills_a_skipped_default() {
    // A static enters with no receiver, so its gap entry is at the plain
    // function's arity. Both bases in one pair of tests.
    must_agree(&format!(
        "{SHADOWED}\
         class Util\n\
         \x20 static fn go(a: integer, d: integer = a * 7, t: string) -> string\n\
         \x20   return tostring(d) .. t\n\
         \x20 end\n\
         end\n\
         Util.go(a: 2, t: \"!\")"
    ));
}

#[test]
fn a_full_arity_call_does_not_take_the_gap_entry() {
    // The gap entry sits at arity `n_params + 1`, one past a complete call.
    // If the two were ever confused, a full call would read its last
    // argument as a mask and overwrite a parameter with a default.
    must_agree(&format!(
        "{SHADOWED}\
         fn f(a: integer, d: integer = a * 2, t: string) -> string\n\
         \x20 return tostring(d) .. t\n\
         end\n\
         f(1, 500, \"!\") .. f(a: 2, d: 600, t: \"?\")"
    ));
}


