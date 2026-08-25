//! Emission peepholes: these assert the *disassembly*, not just engine agreement.

use crate::harness::*;

// -- Phase 5: emission peepholes ------------------------------------------
//
// These assert the *emitted code*, not just agreement. A peephole that
// silently stops firing is invisible to every differential test in this
// file - the program still runs and still agrees, just slower - so the
// disassembly is the only thing that can catch a regression.

/// A newline, so an assertion message can start the listing on its own line.
/// Named because `{NL}` reads better inside these format strings than an
/// escape does next to the Saule source they also contain.
const NL: &str = "
";

/// The disassembly of `src`, for asserting on what was emitted.
fn listing(src: &str) -> String {
    let module = front_end(src);
    let chunk = saule_vm::compile(&module, "peephole.sau", src).expect("compiles");
    saule_vm::disasm::chunk(&chunk)
}

/// Whether `listing` emits `op` anywhere.
///
/// Token-wise, never `contains`: a `CLOSURE` line carries a `; proto[N]`
/// comment and every mnemonic is a substring of some other one - `LTI` is
/// inside `JLTI`, which is the exact pair this file needs to tell apart.
fn emits(listing: &str, op: &str) -> bool {
    listing
        .lines()
        .any(|l| l.split_whitespace().nth(1) == Some(op))
}

/// The lines of one proto, by the name in its header.
///
/// Split on a line start, for the same reason: `; proto[1] <lambda>` is a
/// comment on an instruction of proto 0, and splitting on the bare text
/// truncates the proto that mentions it.
fn proto<'a>(listing: &'a str, name: &str) -> &'a str {
    listing
        .split("
proto[")
        .find(|p| p.lines().next().is_some_and(|h| h.contains(name)))
        .unwrap_or_else(|| panic!("no proto named `{name}` in:
{listing}"))
}

#[test]
fn a_comparison_feeding_an_if_becomes_a_fused_branch() {
    // `LTI` + `TEST` + `JMP` collapses to `JLTI` + `JMP`: the boolean is
    // never materialised. `--profile-bytecode` counted the `LTI TEST` pair
    // 1,028,457 times in `fib` before this.
    let l = listing(
        "fn f(n: integer) -> integer\n\
         \x20 if n < 2 then\n\
         \x20   return 0\n\
         \x20 end\n\
         \x20 return 1\n\
         end\n\
         f(3)",
    );
    // `2` is a literal, so this now goes one further than the fused branch
    // this test was written for: `JLTII` takes the immediate as well, and
    // the `LOADI` that materialised it is gone too. `JLTI` itself is
    // asserted by `every_ordering_operator_has_a_fused_form`, which
    // compares two locals and so has no immediate to fold.
    assert!(emits(&l, "JLTII"), "expected a fused immediate branch:{NL}{l}");
    assert!(!emits(&l, "LTI"), "the materialising form is still emitted:{NL}{l}");
    assert!(!emits(&l, "TEST"), "the boolean is still being tested:{NL}{l}");
    // No `LOADI` assertion here: `return 0` and `return 1` legitimately load
    // literals of their own, and `emits` reads the whole listing. That the
    // comparison's own `LOADI` is gone is what
    // `every_ordering_operator_has_an_immediate_form` checks.
}

#[test]
fn every_ordering_operator_has_a_fused_form() {
    for (op, want) in [
        ("<", "JLTI"),
        ("<=", "JLEI"),
        (">", "JGTI"),
        (">=", "JGEI"),
        ("==", "JEQI"),
        ("!=", "JNEI"),
    ] {
        let src = format!(
            "local a: integer = 1{NL}local b: integer = 2{NL}\
             local r: integer = 0{NL}if a {op} b then{NL} r = 1{NL}end{NL}r"
        );
        let l = listing(&src);
        assert!(emits(&l, want), "`{op}` did not fuse to {want}:{NL}{l}");
        must_agree(&src);
    }
}

#[test]
fn an_unproved_equality_keeps_the_materialising_form() {
    // `EQV` + `TEST` is what makes an `Op*` overload work: `equals` is
    // resolved against the left operand's class, and a fused `JEQI` would
    // compare two instances as integers. The fused forms are gated on a
    // proved numeric kind precisely so this shape never reaches them.
    let l = listing(
        "local a: any = 1{NL}local r: integer = 0{NL}if a == 1 then{NL} r = 1{NL}end{NL}r"
            .replace("{NL}", NL)
            .as_str(),
    );
    assert!(!emits(&l, "JEQI"), "an unproved `==` must not fuse:{NL}{l}");
}

#[test]
fn an_operand_already_in_a_register_is_not_copied() {
    // `MOVE` is the most-executed opcode in every benchmark, and most of
    // them were a parameter copied into a temporary to be an operand.
    let l = listing(
        "fn sub1(n: integer) -> integer\n\
         \x20 return n - 1\n\
         end\n\
         sub1(3)",
    );
    let body = proto(&l, "sub1");
    assert!(
        !emits(body, "MOVE"),
        "`n - 1` copied `n` out of its own register:{NL}{body}"
    );
}

#[test]
fn a_field_access_reads_its_receiver_in_place() {
    let l = listing(
        "class P\n\
         \x20 y: integer = 0\n\
         \x20 fn init(y: integer)\n\
         \x20   self.y = y\n\
         \x20 end\n\
         \x20 fn getY() -> integer\n\
         \x20   return self.y\n\
         \x20 end\n\
         end\n\
         P(3).getY()",
    );
    let body = proto(&l, "P.getY");
    assert!(
        !emits(body, "MOVE"),
        "`self.y` copied `self` out of register 0:{NL}{body}"
    );
}

#[test]
fn a_returned_local_is_still_copied_before_the_frame_pops() {
    // The one place an in-place read is **wrong**, and it is not obvious:
    // `pop_frame` calls `close_upvalues(frame.base)` before moving the
    // results out, and closing does `mem::replace(slot, Value::Nil)`. A
    // `RET1` naming a captured register therefore reads the nil that
    // closing left behind. Whether the register is captured is not even
    // settled when the `return` is compiled - a lambda *below* it can
    // capture it - so the copy stays unconditionally.
    //
    // `a_closure_writes_through_to_its_captured_variable` is what caught
    // this; this test says why the `MOVE` is still there, so nobody
    // "optimises" it away twice.
    let l = listing(
        "fn run() -> integer\n\
         \x20 local n: integer = 0\n\
         \x20 local bump = fn() -> nil\n\
         \x20   n = n + 1\n\
         \x20 end\n\
         \x20 bump()\n\
         \x20 return n\n\
         end\n\
         run()",
    );
    let body = proto(&l, "run(");
    assert!(
        emits(body, "MOVE"),
        "a captured local must be copied before `RET1`:{NL}{body}"
    );
    must_agree(
        "fn run() -> integer\n\
         \x20 local n: integer = 0\n\
         \x20 local bump = fn() -> nil\n\
         \x20   n = n + 1\n\
         \x20 end\n\
         \x20 bump()\n\
         \x20 bump()\n\
         \x20 return n\n\
         end\n\
         run()",
    );
}

#[test]
fn an_in_place_operand_still_sees_the_value_the_oracle_sees() {
    // The hazard in-place reads have to avoid: a captured local is an
    // *open* upvalue pointing at this frame's register, so a closure called
    // between the read and the use would write through it. The purity rule
    // is what rules this out - a call is not pure, so `n + f()` copies `n`
    // first, exactly as the tree-walker evaluates left before right.
    must_agree(
        "fn run() -> integer\n\
         \x20 local n: integer = 1\n\
         \x20 local bump = fn() -> integer\n\
         \x20   n = 100\n\
         \x20   return 10\n\
         \x20 end\n\
         \x20 return n + bump()\n\
         end\n\
         run()",
    );
}

#[test]
fn concat_operands_stay_adjacent() {
    // `CONCAT` is n-ary over a register *range*, so its operands must be
    // consecutive temporaries. Reusing a local's register in place would
    // break the range rather than shorten it, which is why `..` is excluded
    // by name from the in-place rule.
    must_agree(
        "local a: string = \"x\"\n\
         local b: string = \"y\"\n\
         a .. b",
    );
}


// -- Phase 5, slice 2 ------------------------------------------------------

#[test]
fn a_small_integer_literal_folds_into_the_instruction() {
    // `ADDII` / `SUBII` / `MULII` take a signed 8-bit immediate and had
    // never been emitted either. `loop_arith`'s `s + i * 2 - 1` spent two of
    // its six instructions materialising `2` and `1` into registers.
    for (src, want) in [
        ("x + 1", "ADDII"),
        ("x - 1", "SUBII"),
        ("x * 2", "MULII"),
        // `Add` and `Mul` commute, so the literal folds from either side.
        ("1 + x", "ADDII"),
        ("2 * x", "MULII"),
    ] {
        // A *parameter*, not a local: `local x = 7` emits a `LOADI` of its
        // own initializer, which would make the "no literal was
        // materialised" assertion pass or fail for the wrong reason.
        let program =
            format!("fn f(x: integer) -> integer{NL} return {src}{NL}end{NL}f(7)");
        let l = listing(&program);
        let body = proto(&l, "f(");
        assert!(emits(body, want), "`{src}` did not fold to {want}:{NL}{body}");
        assert!(
            !emits(body, "LOADI"),
            "`{src}` still materialised the literal:{NL}{body}"
        );
        must_agree(&program);
    }
}

#[test]
fn subtraction_does_not_fold_a_left_hand_literal() {
    // `SUBII` is `R[B] - sext(C)`, and `1 - x` is not `x - 1`. The
    // commutative fold must not reach it.
    let program = format!("local x: integer = 7{NL}local r: integer = 1 - x{NL}r");
    let l = listing(&program);
    assert!(!emits(&l, "SUBII"), "`1 - x` folded as if it commuted:{NL}{l}");
    must_agree(&program);
}

#[test]
fn a_literal_too_large_for_the_immediate_keeps_the_register_form() {
    // `sext(C)` is 8 bits. Truncating a larger literal into it would be a
    // silently wrong answer, which is the one outcome worse than a slow one.
    for v in ["128", "-129", "1000000"] {
        let program = format!("local x: integer = 7{NL}local r: integer = x + {v}{NL}r");
        let l = listing(&program);
        assert!(!emits(&l, "ADDII"), "`x + {v}` folded into 8 bits:{NL}{l}");
        must_agree(&program);
    }
    // The boundaries themselves do fold.
    for v in ["127", "-128"] {
        let program = format!("local x: integer = 7{NL}local r: integer = x + {v}{NL}r");
        must_agree(&program);
    }
}

#[test]
fn float_arithmetic_has_no_immediate_form() {
    let program = format!("local x: float = 7.0{NL}local r: float = x + 1.0{NL}r");
    let l = listing(&program);
    assert!(!emits(&l, "ADDII"), "a float folded into an integer immediate:{NL}{l}");
    must_agree(&program);
}

#[test]
fn arithmetic_over_pure_operands_is_itself_pure() {
    // `s + i * 2` reads `s` in place because `i * 2` cannot run user code:
    // both its operands are proved integers, so it is a typed opcode rather
    // than `ARITHX`. This is `loop_arith`'s inner loop and nothing else.
    let program = format!(
        "local s: integer = 0{NL}local i: integer = 3{NL}\
         local r: integer = s + i * 2 - 1{NL}r"
    );
    let l = listing(&program);
    assert!(!emits(&l, "MOVE"), "a pure arithmetic operand still copied:{NL}{l}");
    must_agree(&program);
}

#[test]
fn an_unproved_operand_is_not_treated_as_pure() {
    // Without a proved numeric kind the operator compiles to `ARITHX`,
    // which calls `ops::binary` - and that dispatches an `Op*` overload,
    // i.e. user code, in the middle of what the purity rule promises runs
    // none. It must also not fold a literal into an immediate, because
    // `ADDII` is an integer instruction and `a` here need not be one.
    let program = format!(
        "local a: any = 7{NL}local r: any = a + 1{NL}tostring(r)"
    );
    let l = listing(&program);
    assert!(!emits(&l, "ADDII"), "an unproved `+` folded an immediate:{NL}{l}");
    assert!(emits(&l, "ARITHX"), "expected the dynamic form:{NL}{l}");
    must_agree(&program);
}

#[test]
fn an_if_with_no_else_does_not_end_in_a_jump_to_the_next_instruction() {
    // §17's fourth peephole. The loop that emitted these carried a comment
    // saying "only worth a jump to the end when something follows" and
    // emitted one unconditionally.
    let program = format!(
        "local x: integer = 1{NL}local r: integer = 0{NL}\
         if x < 2 then{NL} r = 1{NL}end{NL}r"
    );
    let l = listing(&program);
    let jumps = l
        .lines()
        .filter(|line| line.split_whitespace().nth(1) == Some("JMP"))
        .count();
    assert_eq!(jumps, 1, "expected only the branch's own jump:{NL}{l}");
    must_agree(&program);
}

#[test]
fn a_conditional_return_still_gets_a_terminating_ret() {
    // **The bug dropping that jump caused**, and it is not obvious. A proto
    // gets a synthesized `RET0` when control can reach the end of its code
    // array, and `pop_function` tested that with "is the last instruction a
    // return". Those are different questions: a forward jump patched to
    // `code.len()` lands one *past* the last instruction. While every `if`
    // arm ended in an unconditional jump the two coincided; the moment they
    // stopped, `fn f() if c then return a end end` ran off the end of the
    // proto with `ran off the end of ... - proto has no terminating RET`.
    //
    // Caught by `run_examples_diff.sh` on `json_usage`, not by any fixture:
    // the shape needs a function whose *last* statement is a conditional
    // return, which is ordinary in real code and absent from small tests.
    // `FuncCtx::max_patch_target` is the fix.
    must_agree(
        "fn pick(n: integer) -> integer?
           if n > 0 then
             return 1
           end
         end
         local a: integer = pick(5) ?? 0
         local b: integer = pick(-5) ?? 0
         a * 10 + b",
    );
}

#[test]
fn a_cast_and_an_index_read_their_operands_in_place() {
    // `sort`'s comparator, `(a as integer)! < (b as integer)!` on untyped
    // lambda parameters: 46% of that benchmark is `CASTCHK` + `UNWRAPNIL`,
    // and every one of them was preceded by a `MOVE`.
    let l = listing(
        "local t: table<integer> = {3, 1, 2}\n\
         Table.sort(t, (a, b) => (a as integer)! < (b as integer)!)\n\
         t[1] .. \",\" .. t[3]",
    );
    let body = proto(&l, "<lambda>");
    assert!(
        !emits(body, "MOVE"),
        "the comparator still copies its parameters:{NL}{body}"
    );
    must_agree(
        "local t: table<integer> = {3, 1, 2}\n\
         Table.sort(t, (a, b) => (a as integer)! < (b as integer)!)\n\
         t[1] .. \",\" .. t[3]",
    );
}

// -- §16 superinstructions -------------------------------------------------

#[test]
fn a_cast_that_is_immediately_unwrapped_becomes_one_instruction() {
    // The first superinstruction in this instruction set, and the only
    // candidate a profile has ever supported: `--profile-bytecode` counts
    // `CASTCHK UNWRAPNIL` as an adjacent pair 6,665,964 times in
    // `benchmarks/sau/sort.sau` — 46% of the program between the two halves.
    let l = listing(
        "local t: table<integer> = {3, 1, 2}\n\
         Table.sort(t, (a, b) => (a as integer)! < (b as integer)!)\n\
         t[1] .. \",\" .. t[3]",
    );
    let body = proto(&l, "<lambda>");
    assert!(emits(body, "CASTUNWRAP"), "the pair did not fuse:{NL}{body}");
    assert!(!emits(body, "CASTCHK"), "the cast is still separate:{NL}{body}");
    assert!(!emits(body, "UNWRAPNIL"), "the unwrap is still separate:{NL}{body}");
}

#[test]
fn a_cast_without_an_unwrap_keeps_the_nil_yielding_form() {
    // `x as T` on its own must not raise: the static type is `T?` and the
    // caller is expected to handle the nil. Fusing unconditionally would
    // turn every failed cast in the language into an error.
    let program = "local x: any = \"no\"\nlocal r: integer? = x as integer\nr ?? -1";
    let l = listing(program);
    assert!(emits(&l, "CASTCHK"), "expected the nil-yielding form:{NL}{l}");
    assert!(!emits(&l, "CASTUNWRAP"), "a bare cast fused into the raising form:{NL}{l}");
    must_agree(program);
}

#[test]
fn an_unwrap_that_is_not_a_cast_keeps_its_own_opcode() {
    let program = "local x: integer? = 7\nx!";
    let l = listing(program);
    assert!(emits(&l, "UNWRAPNIL"), "expected a plain unwrap:{NL}{l}");
    assert!(!emits(&l, "CASTUNWRAP"), "a plain unwrap fused with nothing:{NL}{l}");
    must_agree(program);
}

#[test]
fn a_fused_cast_raises_exactly_where_the_pair_did() {
    // The failure path is `UNWRAPNIL`'s, not a new one, and the span it
    // reports is the `!`'s — which is the span `UNWRAPNIL` carried. Both
    // halves matter: a cast that *succeeds* must still yield the value, and
    // one that fails must raise rather than yield nil.
    must_agree(
        "local ok: any = 7\n\
         local good: integer = (ok as integer)!\n\
         local caught: string = \"none\"\n\
         try\n\
         \x20 local bad: any = \"not a number\"\n\
         \x20 local n: integer = (bad as integer)!\n\
         \x20 caught = \"unreachable \" .. n\n\
         catch e: any\n\
         \x20 caught = \"raised\"\n\
         end\n\
         good .. \"/\" .. caught",
    );
}

#[test]
fn a_fused_cast_still_walks_the_type_it_was_given() {
    // `CASTUNWRAP` calls the same `eval::expr::cast::cast` `CASTCHK` does,
    // so the deep cases come along: a `table<integer>` is checked
    // elementwise and a class cast walks the inheritance chain. Reusing the
    // oracle's function is what makes that true by construction rather than
    // by care.
    must_agree(
        "local t: any = {1, 2, 3}\n\
         local nums: table<integer> = (t as table<integer>)!\n\
         nums[1] + nums[3]",
    );
    must_agree(
        "class Animal\n\
         \x20 name: string = \"a\"\n\
         end\n\
         class Dog extends Animal\n\
         end\n\
         local d: any = Dog()\n\
         local a: Animal = (d as Animal)!\n\
         a.name",
    );
}



// -- Phase 5, slice 3: immediate compares and the peephole pass -------------

#[test]
fn every_ordering_operator_has_an_immediate_form() {
    // The comparison counterpart of `a_small_integer_literal_folds_into_the_
    // instruction`. `fib`'s `n < 2` runs once per call and spent an
    // instruction and a register materialising the `2`.
    for (op, want) in [
        ("<", "JLTII"),
        ("<=", "JLEII"),
        (">", "JGTII"),
        (">=", "JGEII"),
        ("==", "JEQII"),
        ("!=", "JNEII"),
    ] {
        let src = format!(
            "local a: integer = 1{NL}\
             local r: integer = 0{NL}if a {op} 2 then{NL} r = 1{NL}end{NL}r"
        );
        let l = listing(&src);
        assert!(emits(&l, want), "`{op} 2` did not fold to {want}:{NL}{l}");
        must_agree(&src);
    }
}

#[test]
fn a_literal_on_the_left_mirrors_the_comparison() {
    // `2 < a` is `a > 2`, so it folds against the *right* operand with the
    // mirrored opcode rather than needing a second family of "immediate on
    // the left" instructions. Getting the mirror backwards inverts the
    // branch and is invisible in the listing, which is what `must_agree` is
    // here for.
    for (op, want) in [
        ("<", "JGTII"),
        ("<=", "JGEII"),
        (">", "JLTII"),
        (">=", "JLEII"),
        ("==", "JEQII"),
        ("!=", "JNEII"),
    ] {
        let src = format!(
            "local a: integer = 1{NL}\
             local r: integer = 0{NL}if 2 {op} a then{NL} r = 1{NL}end{NL}r"
        );
        let l = listing(&src);
        assert!(emits(&l, want), "`2 {op} a` did not mirror to {want}:{NL}{l}");
        must_agree(&src);
    }
}

#[test]
fn a_literal_too_wide_for_a_byte_keeps_the_register_form() {
    // `sext(C)` is an `i64`, so truncating a wider literal would compare
    // against a different number - a wrong answer, which this project
    // treats as worse than a slow one. Both ends of the range, and one past
    // each.
    for (lit, want) in [
        ("127", "JLTII"),
        ("128", "JLTI"),
        ("-128", "JLTII"),
        ("-129", "JLTI"),
        ("1000", "JLTI"),
    ] {
        let src = format!(
            "local a: integer = 1{NL}\
             local r: integer = 0{NL}if a < {lit} then{NL} r = 1{NL}end{NL}r"
        );
        let l = listing(&src);
        assert!(emits(&l, want), "`a < {lit}` should emit {want}:{NL}{l}");
        must_agree(&src);
    }
}

#[test]
fn an_unproved_comparison_does_not_fold_a_literal() {
    // The immediate forms read their register as an `i64`. An `any` operand
    // may be an instance with a `compare` overload, and the gate is the
    // same proved-numeric-kind test the register forms use.
    let src =
        format!("local a: any = 1{NL}local r: integer = 0{NL}if a < 2 then{NL} r = 1{NL}end{NL}r");
    let l = listing(&src);
    assert!(!emits(&l, "JLTII"), "an unproved `<` must not fold:{NL}{l}");
    must_agree(&src);
}

#[test]
fn a_float_comparison_does_not_fold_a_literal() {
    // There is no float immediate, for the same reason `ADDF` has none: the
    // operand is a byte.
    let src = format!(
        "local a: float = 1.0{NL}local r: integer = 0{NL}\
         if a < 2.0 then{NL} r = 1{NL}end{NL}r"
    );
    let l = listing(&src);
    assert!(emits(&l, "JLTF"), "a float compare should still fuse:{NL}{l}");
    assert!(
        !emits(&l, "JLTII"),
        "a float folded into an integer immediate:{NL}{l}"
    );
    must_agree(&src);
}

#[test]
fn a_returned_local_is_not_copied_when_the_frame_cannot_capture() {
    // The peephole's reason to exist: the `MOVE` and the `RET1` are emitted
    // by different parts of the compiler, and neither can see the other's
    // operand at the time it is written.
    //
    // And it is a *pass* rather than an emission rule precisely because of
    // what `a_returned_local_is_still_copied_before_the_frame_pops`
    // documents: whether a register is captured is not settled when the
    // `return` is compiled, since a lambda below it can still capture it.
    // By the time the body is finished, every `CLOSURE` it will ever hold
    // has been emitted, so the question finally has an answer.
    const SRC: &str = "fn pick(n: integer) -> integer\n\
         \x20 local out: integer = n + 1\n\
         \x20 return out\n\
         end\n\
         pick(3)";
    let l = listing(SRC);
    let body = proto(&l, "pick(");
    assert!(
        !emits(body, "MOVE"),
        "the return copy survived the peephole:{NL}{body}"
    );
    must_agree(SRC);
}

#[test]
fn the_peephole_relocates_the_jumps_it_moves() {
    // Deleting a word renumbers every instruction after it, and a jump
    // displacement is *relative*: a deletion between a jump and its target
    // has to shorten it. `fib`'s own shape is the case - the `JMP` over the
    // early return jumps across the `MOVE` this pass removes - and an
    // unrelocated displacement lands one instruction late, inside the code
    // it was meant to skip.
    must_agree(
        "fn fib(n: integer) -> integer\n\
         \x20 if n < 2 then\n\
         \x20   return n\n\
         \x20 end\n\
         \x20 return fib(n - 1) + fib(n - 2)\n\
         end\n\
         fib(12)",
    );
    // Backward as well as forward: a loop's back edge crosses whatever the
    // pass removed inside the body.
    must_agree(
        "fn count(n: integer) -> integer\n\
         \x20 local s: integer = 0\n\
         \x20 for i: integer = 1, n do\n\
         \x20   if i > 2 then\n\
         \x20     s = s + i\n\
         \x20   end\n\
         \x20 end\n\
         \x20 return s\n\
         end\n\
         count(10)",
    );
}

#[test]
fn the_peephole_relocates_a_match_jump_table() {
    // `SWITCH` targets live in the **chunk**, shared by every function in
    // the module, and they are absolute instruction indices into the proto
    // that emitted them. So the pass relocates the tables this function
    // owns and leaves a sibling's numbering alone - a table renumbered
    // against the wrong body jumps into the middle of an arm.
    must_agree(
        "enum Color\n\
         \x20 Red\n\
         \x20 Green\n\
         \x20 Blue\n\
         end\n\
         fn name(c: Color) -> string\n\
         \x20 local out: string = match c\n\
         \x20   case Color.Red then \"r\"\n\
         \x20   case Color.Green then \"g\"\n\
         \x20   case Color.Blue then \"b\"\n\
         \x20 end\n\
         \x20 return out\n\
         end\n\
         fn other(n: integer) -> integer\n\
         \x20 local m: integer = n + 1\n\
         \x20 return m\n\
         end\n\
         name(Color.Green) .. other(1)",
    );
}

#[test]
fn the_peephole_relocates_a_handler_range() {
    // A `try` body is a pc *range* plus a catch entry, all three absolute.
    // A deletion inside the body that did not move `pc_end` would leave a
    // throw at the end of the body outside the handler meant to catch it.
    must_agree(
        "fn risky(n: integer) -> string\n\
         \x20 local out: string = \"none\"\n\
         \x20 try\n\
         \x20   local m: integer = n + 1\n\
         \x20   if m > 2 then\n\
         \x20     throw \"too big\"\n\
         \x20   end\n\
         \x20   out = \"ok\"\n\
         \x20 catch e: any\n\
         \x20   out = \"caught\"\n\
         \x20 end\n\
         \x20 return out\n\
         end\n\
         risky(5) .. risky(0)",
    );
}

#[test]
fn a_fault_after_a_peephole_still_blames_the_right_line() {
    // The line table names instructions by index too, and it is the only
    // one whose breakage is invisible to a passing program: the answer is
    // right and the error message points at the wrong source. `must_agree`
    // compares error *text*, so a shifted span fails here.
    must_agree(
        "fn get(t: table<integer>, i: integer) -> integer\n\
         \x20 local v: integer = t[i]!\n\
         \x20 return v\n\
         end\n\
         local t: table<integer> = {1, 2}\n\
         get(t, 5)",
    );
}


// -- Phase 5, slice 4: JEQK, and `and` in branch position -------------------

#[test]
fn an_equality_against_a_constant_becomes_a_fused_branch() {
    // `JEQK` had been in the instruction set since Phase 1, was checked by
    // the verifier, had a verifier test of its own — and nothing ever
    // emitted one. `if c == "{"` compiled to `LOADK` + `EQV` + `TEST` +
    // `JMP`: four words to ask one question, and `json` asked it six
    // million times.
    let src = format!(
        "local c: string = \"x\"{NL}local r: integer = 0{NL}\
         if c == \"{{\" then{NL} r = 1{NL}end{NL}r"
    );
    let l = listing(&src);
    assert!(emits(&l, "JEQK"), "a constant `==` did not fuse:{NL}{l}");
    assert!(!emits(&l, "EQV"), "the materialising form is still emitted:{NL}{l}");
    assert!(!emits(&l, "TEST"), "the boolean is still being tested:{NL}{l}");
    must_agree(&src);
}

#[test]
fn a_constant_equality_folds_from_either_side() {
    // `==` commutes and `JEQK` reads `R[A] == K[C]`, so the constant folds
    // from the left with no mirrored opcode — unlike the ordering
    // comparisons, which need one.
    let src = format!(
        "local c: string = \"x\"{NL}local r: integer = 0{NL}\
         if \"x\" == c then{NL} r = 1{NL}end{NL}r"
    );
    let l = listing(&src);
    assert!(emits(&l, "JEQK"), "a left-hand constant did not fold:{NL}{l}");
    must_agree(&src);
}

#[test]
fn an_inequality_against_a_constant_keeps_the_materialising_form() {
    // `JEQK` skips *on equality* and has no `!=` counterpart, so inverting
    // it would need a second jump to undo the skip — three words where the
    // materialising path takes four. Not worth a second opcode until a
    // profile asks for one.
    let src = format!(
        "local c: string = \"x\"{NL}local r: integer = 0{NL}\
         if c != \"y\" then{NL} r = 1{NL}end{NL}r"
    );
    let l = listing(&src);
    assert!(!emits(&l, "JEQK"), "`!=` must not fuse to an equality:{NL}{l}");
    must_agree(&src);
}

#[test]
fn a_class_receiver_compared_to_a_constant_matches_the_materialising_form() {
    // `constant_compare_jump` carries an `equals`-overload guard copied
    // from `binary_to`, and today that guard **cannot fire** — which is
    // worth pinning rather than leaving to be rediscovered.
    //
    // `saule-typeck` rejects `Money == 2` outright (`DisjointEquality`),
    // so a class-typed operand never meets a literal of another type. And
    // an *optional* class type does not resolve through `class_of_expr`,
    // so `m == nil` dispatches no overload here — and none in `binary_to`
    // either, which asks the same question. Both therefore fall back to
    // `Value`'s own equality, which is exactly what makes substituting
    // `JEQK` for `EQV` sound rather than merely plausible.
    //
    // If `class_of_expr` ever learns about optionals, this is where the
    // guard has to be looked at again.
    must_agree(
        "class Money\n\
         \x20 cents: integer = 0\n\
         \x20 fn init(cents: integer)\n\
         \x20   self.cents = cents\n\
         \x20 end\n\
         \x20 fn equals(other: any) -> boolean\n\
         \x20   return true\n\
         \x20 end\n\
         end\n\
         local m: Money? = Money(1)\n\
         local r: integer = 0\n\
         if m == nil then\n\
         \x20 r = 1\n\
         end\n\
         r",
    );
}

#[test]
fn an_and_in_branch_position_tests_each_conjunct() {
    // `and` compiled as an *expression* materialises a value through
    // `TESTSET` and the branch then tests that — and a comparison inside it
    // never reached `fused_compare_jump` at all, because that only saw the
    // top-level expression. `--profile-bytecode` counted the resulting
    // `LEI TEST` pair 4,541,139 times in `json`, 4.1% of the program.
    let src = format!(
        "local a: integer = 1{NL}local b: integer = 2{NL}local r: integer = 0{NL}\
         if a <= b and b <= 9 then{NL} r = 1{NL}end{NL}r"
    );
    let l = listing(&src);
    assert!(emits(&l, "JLEI"), "the first conjunct did not fuse:{NL}{l}");
    assert!(emits(&l, "JLEII"), "the second conjunct did not fuse:{NL}{l}");
    assert!(!emits(&l, "TESTSET"), "the `and` still materialises:{NL}{l}");
    assert!(!emits(&l, "LEI"), "a conjunct still materialises a bool:{NL}{l}");
    must_agree(&src);
}

#[test]
fn an_and_still_short_circuits_its_right_hand_side() {
    // The jump out of the left conjunct is emitted *before* the right
    // one's code, so a false left leaves without evaluating the right. A
    // side effect on the right is what makes that observable rather than
    // merely faster, and the oracle evaluates it the same way.
    must_agree(
        "local calls: integer = 0\n\
         fn bump() -> boolean\n\
         \x20 calls = calls + 1\n\
         \x20 return true\n\
         end\n\
         local r: integer = 0\n\
         if false and bump() then\n\
         \x20 r = 1\n\
         end\n\
         if true and bump() then\n\
         \x20 r = 2\n\
         end\n\
         r .. \"/\" .. calls",
    );
}

#[test]
fn a_chain_of_ands_leaves_from_every_conjunct() {
    // `a and b and c` parses as `(a and b) and c`, so the split recurses
    // and all three tests jump to the same false target. Each arm is
    // exercised in turn, because a mispatched label shows up as one
    // specific conjunct being skipped.
    must_agree(
        "fn check(a: integer, b: integer, c: integer) -> string\n\
         \x20 if a > 0 and b > 0 and c > 0 then\n\
         \x20   return \"all\"\n\
         \x20 end\n\
         \x20 return \"no\"\n\
         end\n\
         check(1, 1, 1) .. check(0, 1, 1) .. check(1, 0, 1) .. check(1, 1, 0)",
    );
}

#[test]
fn an_and_in_a_while_condition_still_leaves_the_loop() {
    // `while` patches the condition's jumps *after* the back edge, so a
    // second conjunct's label has to survive being patched later than the
    // first. This is `json`'s own scanner shape.
    must_agree(
        "local s: string = \"aaab\"\n\
         local pos: integer = 1\n\
         local n: integer = 4\n\
         while pos <= n and String.sub(s, pos, pos) == \"a\" do\n\
         \x20 pos = pos + 1\n\
         end\n\
         pos",
    );
}

#[test]
fn an_and_as_a_value_still_materialises() {
    // Only *branch* position is split. `and` is still an expression that
    // yields a value everywhere else, and Saule's yields the operand rather
    // than a boolean — so this is not a shape the split may quietly change.
    must_agree(
        "local a: any = 0\n\
         local b: any = \"kept\"\n\
         local x: any = a and b\n\
         local y: any = b and a\n\
         tostring(x) .. \"/\" .. tostring(y)",
    );
}

#[test]
fn an_or_in_branch_position_is_left_alone() {
    // Not symmetry for its own sake: `a or b` needs a jump taken when `a`
    // is **true**, and inverting a fused comparison at the `BinOp` level is
    // wrong for floats — `!(a < b)` is true when either side is NaN, where
    // `a >= b` is false. No profile asks for it, so `or` keeps the
    // materialising path and this pins that it still works.
    must_agree(
        "fn check(a: integer, b: integer) -> string\n\
         \x20 if a > 0 or b > 0 then\n\
         \x20   return \"some\"\n\
         \x20 end\n\
         \x20 return \"none\"\n\
         end\n\
         check(1, 0) .. check(0, 1) .. check(0, 0) .. check(1, 1)",
    );
}
