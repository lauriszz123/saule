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
fn an_import_without_a_program_driver_still_refuses() {
    // Compiling one module on its own cannot bind an imported name: the
    // resolver gives it a module slot, and nothing would ever write to that
    // slot. Emitting a `GETMOD` against it would read `nil` — a wrong
    // answer with no symptom. Only `program::compile`, which resolves the
    // whole import graph first, may compile an `import` to nothing.
    //
    // This is also the canary for the *shape* of a refusal — it names the
    // construct and carries a span, so the error the user sees points
    // somewhere. It took over from `t[idx()] += 1`, the last refusal a
    // valid program could reach, once compound assignment learned to pin
    // its target's sub-expressions in registers.
    let src = "import Json from \"json\"\n1";
    let module = front_end(src);
    match saule_vm::compile(&module, "x.sau", src) {
        Err(saule_vm::CompileError::Unsupported { thing, span }) => {
            assert_eq!(thing, "an import in a program with no file behind it");
            assert!(span.start < span.end, "the refusal must point somewhere");
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


