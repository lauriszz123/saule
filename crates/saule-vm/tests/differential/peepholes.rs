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
    assert!(emits(&l, "JLTI"), "expected a fused branch:{NL}{l}");
    assert!(!emits(&l, "LTI"), "the materialising form is still emitted:{NL}{l}");
    assert!(!emits(&l, "TEST"), "the boolean is still being tested:{NL}{l}");
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

