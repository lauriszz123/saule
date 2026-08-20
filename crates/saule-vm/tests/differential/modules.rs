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
    // `a`, not the callee's — so this must keep refusing rather than
    // quietly binding the wrong value.
    //
    // `agree` rather than `must_agree`: a refusal is the designed outcome,
    // and it means the VM never runs this, so there is nothing to compare.
    // What is asserted is that the tree-walker's answer is the one a
    // materializing compiler would have got *wrong* — 200 from the caller's
    // `a`, not 6 from the callee's.
    let src = "local a: integer = 100\n\
               fn f(a: integer, d: integer = a * 2, t: string) -> string\n\
               \x20 return tostring(d) .. t\n\
               end\n\
               f(a: 3, t: \"!\")";
    let module = front_end(src);
    assert!(
        matches!(
            saule_vm::compile(&module, "diff.sau", src),
            Err(saule_vm::CompileError::Unsupported { .. })
        ),
        "a non-literal skipped default must refuse rather than be materialized at the call site"
    );
    assert_eq!(tree_walker(&module), Outcome::Value("string:6!".into()));
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


