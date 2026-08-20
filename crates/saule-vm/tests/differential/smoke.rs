//! Whole-program smoke tests, and the `Unsupported` fallback signal itself.

use crate::harness::*;

// ── the pieces working together ───────────────────────────────────────────

#[test]
fn a_larger_program_matches() {
    must_agree(
        "local total: integer = 0\n\
         local count: integer = 0\n\
         for i = 1, 50 do\n\
           if i % 3 == 0 then\n\
             total = total + i\n\
             count = count + 1\n\
           elseif i % 5 == 0 then\n\
             total = total + i * 2\n\
           end\n\
         end\n\
         total * 1000 + count",
    );
}

#[test]
fn unsupported_constructs_report_rather_than_miscompile() {
    // The contract that makes `--vm` usable before it is finished: anything
    // codegen cannot do yet is refused by name, never guessed at.
    //
    // The construct standing in for that here is a **compound assignment
    // whose target cannot be evaluated only once** — `t[idx()] += 1`.
    //
    // This one is deliberately different from its four predecessors
    // (`import`, then a pipe, then a tuple pattern, then a compound
    // assignment to a member), every one of which had to be repointed as the
    // feature landed. This refusal is *principled* rather than unfinished:
    // the target appears twice in what the compiler builds, so re-reading it
    // has to be unobservable, and a side-effecting subscript never will be.
    // It should therefore stay put — and if it is ever lifted, that will be
    // because compound assignment was rebuilt to resolve its target into
    // registers once, which is a change worth failing a canary over.
    //
    // The assertion is about the *shape* of the refusal: it names the
    // construct and carries a span, so the CLI can fall back and say why.
    let src = "local t: table<integer> = {1, 2}\n\
               fn idx() -> integer\n\
               \x20 return 1\n\
               end\n\
               t[idx()] += 1\n\
               t[1]";
    let module = front_end(src);
    match saule_vm::compile(&module, "x.sau", src) {
        Err(saule_vm::CompileError::Unsupported { thing, span }) => {
            assert_eq!(
                thing,
                "a compound assignment whose target cannot be evaluated only once"
            );
            assert!(span.start < span.end, "the refusal must point somewhere");
        }
        other => panic!("expected a clean Unsupported, got {other:?}"),
    }
}

#[test]
fn an_import_without_a_program_driver_still_refuses() {
    // Compiling one module on its own cannot bind an imported name: the
    // resolver gives it a module slot, and nothing would ever write to that
    // slot. Emitting a `GETMOD` against it would read `nil` — a wrong
    // answer with no symptom. Only `program::compile`, which resolves the
    // whole import graph first, may compile an `import` to nothing.
    let src = "import Json from \"json\"\n1";
    let module = front_end(src);
    match saule_vm::compile(&module, "x.sau", src) {
        Err(saule_vm::CompileError::Unsupported { thing, .. }) => {
            assert_eq!(thing, "an import declaration");
        }
        other => panic!("expected a clean Unsupported, got {other:?}"),
    }
}


// ── everything together ───────────────────────────────────────────────────

#[test]
fn a_program_with_functions_and_loops_matches() {
    must_agree(
        "fn isPrime(n: integer) -> boolean\n\
         \x20 if n < 2 then return false end\n\
         \x20 local i: integer = 2\n\
         \x20 while i * i <= n do\n\
         \x20   if n % i == 0 then return false end\n\
         \x20   i = i + 1\n\
         \x20 end\n\
         \x20 return true\n\
         end\n\
         local count: integer = 0\n\
         local sum: integer = 0\n\
         for n = 1, 200 do\n\
           if isPrime(n) then\n\
             count = count + 1\n\
             sum = sum + n\n\
           end\n\
         end\n\
         sum * 1000 + count",
    );
}


