//! A native calling into compiled bytecode, and back.

use crate::harness::*;

// ── Re-entrancy ──────────────────────────────────────────────────────────
//
// One root cause with several symptoms. `Value::VmFunction` used to be
// uncallable from the runtime and a VM-built `ClassObject` used to carry an
// empty method map, so every path where a native has to call a user
// function on a value hit a wall — a sort comparator, an operator overload,
// a `toString`. Each test below is one of those paths, and `must_agree`
// fails if the compiler declines, which is the point: a refusal used to
// hide these.

#[test]
fn a_native_invokes_a_bytecode_comparator() {
    // `Table.sort`'s comparator, the case that kept `sort.sau` on the
    // tree-walker. The native calls `call_value_multi`, which now has an
    // arm that runs a fresh `Vm` over the caller's shared state.
    must_agree(
        "local t: table<integer, integer> = {5, 3, 9, 1}\n\
         Table.sort(t, (a: integer, b: integer) => a < b)\n\
         local r: string = t[1] .. \",\" .. t[2] .. \",\" .. t[3] .. \",\" .. t[4]\nr",
    );
    // Descending, so a comparator that is actually consulted is the only
    // way to pass: a no-op callback would leave the ascending order above.
    must_agree(
        "local t: table<integer, integer> = {5, 3, 9, 1}\n\
         Table.sort(t, (a: integer, b: integer) => a > b)\n\
         local r: string = t[1] .. \",\" .. t[2] .. \",\" .. t[3] .. \",\" .. t[4]\nr",
    );
}

#[test]
fn a_comparator_closure_captures_its_environment() {
    // The callback runs on a *fresh* register file but over the same shared
    // half, and it reaches its captured `flip` through an upvalue cell that
    // outlived the frame that created it.
    must_agree(
        "local flip: boolean = true\n\
         local cmp = fn(a: integer, b: integer) -> boolean\n\
         \x20 if flip then\n\
         \x20   return a > b\n\
         \x20 end\n\
         \x20 return a < b\n\
         end\n\
         local t: table<integer, integer> = {2, 8, 4}\n\
         Table.sort(t, cmp)\n\
         local r: string = t[1] .. \",\" .. t[2] .. \",\" .. t[3]\nr",
    );
}

#[test]
fn a_tostring_overload_is_honoured_by_concatenation() {
    // The worst failure this project could ship, and the one that was live:
    // `display_value` asked the class for a `toString`, a VM-built class
    // answered no, and `..` printed `<instance of Money>` — **with no
    // error**. Caught by `SAULE_DIFF=1`, not by any exit status.
    let money = "class Money implements OpToString\n\
                 \x20 local amount: integer\n\
                 \x20 fn init(a: integer)\n\
                 \x20   self.amount = a\n\
                 \x20 end\n\
                 \x20 fn toString() -> string\n\
                 \x20   return \"$\" .. self.amount\n\
                 \x20 end\n\
                 end\n";
    must_agree(&format!(
        "{money}local m: Money = Money(7)\nlocal r: string = \"cost: \" .. m\nr"
    ));
    must_agree(&format!(
        "{money}local m: Money = Money(7)\nlocal r: string = tostring(m)\nr"
    ));
    // Nested in a table, the *structural* rendering wins — `display_value`
    // applies to the value itself, not to values inside a table. The two
    // engines have to agree about that boundary too.
    must_agree(&format!(
        "{money}local t: table<integer, Money> = {{Money(1)}}\n\
         local r: string = \"\" .. t[1]\nr"
    ));
}

#[test]
fn a_tostring_overload_runs_exactly_once_per_operand() {
    // `CONCAT` used to render each operand twice — once to measure the
    // result's length, once to build it. Harmless while rendering was pure;
    // an overload is user code, so a second pass would run its side effects
    // twice. Counted rather than inferred.
    must_agree(
        "class Loud implements OpToString\n\
         \x20 static calls: integer = 0\n\
         \x20 fn toString() -> string\n\
         \x20   Loud.calls = Loud.calls + 1\n\
         \x20   return \"x\"\n\
         \x20 end\n\
         end\n\
         local a: Loud = Loud()\n\
         local b: Loud = Loud()\n\
         local s: string = a .. \"-\" .. b\n\
         local r: integer = Loud.calls\nr",
    );
}

#[test]
fn an_operator_overload_resolves_on_an_unproved_receiver() {
    // When the front end proved the operand's class the compiler picks the
    // overload itself. When it did not — a call result, here — `ARITHX`
    // falls through to `ops::binary`, which looks the overload up on the
    // runtime class. That lookup is the one that used to find an empty map.
    must_agree(
        "class Money implements OpAdd<Money, Money>, OpToString\n\
         \x20 local amount: integer\n\
         \x20 fn init(a: integer)\n\
         \x20   self.amount = a\n\
         \x20 end\n\
         \x20 fn add(other: Money) -> Money\n\
         \x20   return Money(self.amount + other.amount)\n\
         \x20 end\n\
         \x20 fn toString() -> string\n\
         \x20   return \"$\" .. self.amount\n\
         \x20 end\n\
         end\n\
         fn make(n: integer) -> Money\n\
         \x20 return Money(n)\n\
         end\n\
         local r: string = \"\" .. (make(2) + make(40))\nr",
    );
}

#[test]
fn an_inherited_method_is_reachable_through_the_runtime_class() {
    // The method map the VM builds comes from `vindex` and `vtable`, both
    // of which are prefix-extensions of the parent's — so an inherited,
    // non-overridden `toString` is one probe away, exactly as it is on a
    // tree-walker class. Copying only a class's *own* methods would leave
    // this one unreachable and silently fall back to `<instance of Dog>`.
    must_agree(
        "class Animal implements OpToString\n\
         \x20 fn toString() -> string\n\
         \x20   return \"an animal\"\n\
         \x20 end\n\
         end\n\
         class Dog extends Animal\n\
         end\n\
         local d: Dog = Dog()\n\
         local r: string = \"\" .. d\nr",
    );
}

#[test]
fn a_callback_can_itself_reach_back_into_the_vm() {
    // Two levels of re-entrancy: the outer comparator is called from a
    // native, and *it* calls a native that calls another comparator. Each
    // level is a fresh `Vm` over the same shared half, so this is where a
    // per-invocation piece wrongly left in `VmShared` — the register file,
    // the frame list, the open upvalues — would corrupt the level below it
    // rather than merely be slow.
    must_agree(
        "local inner: table<integer, integer> = {3, 1, 2}\n\
         fn outer(a: integer, b: integer) -> boolean\n\
         \x20 Table.sort(inner, (x: integer, y: integer) => x < y)\n\
         \x20 return a < b\n\
         end\n\
         local t: table<integer, integer> = {9, 4, 6}\n\
         Table.sort(t, outer)\n\
         local r: string = t[1] .. \",\" .. t[2] .. \",\" .. t[3] .. \"|\" ..\n\
         \x20 inner[1] .. \",\" .. inner[2] .. \",\" .. inner[3]\nr",
    );
}

#[test]
fn the_recursion_guard_still_unwinds_after_re_entrant_calls() {
    // A guard that leaked a level per callback would shrink every later
    // program's budget — and because the counter is per *thread*, the
    // symptom would surface as an unrelated test failing later in this
    // binary. Sorting drives many comparator calls, so a drift of one per
    // call would be obvious immediately after.
    //
    // The unbounded case — a comparator that sorts with itself forever —
    // is pinned by `tests/ui/stack_overflow_reentrant.sau` instead. It
    // needs `MAX_EVAL_DEPTH` native frames of real stack, which is more
    // than libtest's 2 MiB test thread has; that fixture runs `saule` as a
    // process, on a main thread, which is the configuration users get.
    //
    // **Runs on its own thread**, and that is not incidental. `depth(60)` is
    // 60 nested Saule frames, which the tree-walker spends many Rust frames
    // apiece on; in a debug build that overflows libtest's 2 MiB and aborts
    // the *process*, so the whole binary reported
    // `STATUS_STACK_OVERFLOW` and no test result at all. The depth is what
    // the assertion needs, so the stack is what had to change.
    on_a_real_stack(|| {
        must_agree(
            "local t: table<integer, integer> = {5, 2, 9, 1, 7, 3}\n\
             Table.sort(t, (a: integer, b: integer) => a < b)\n\
             fn depth(n: integer) -> integer\n\
             \x20 if n <= 0 then\n\
             \x20   return 0\n\
             \x20 end\n\
             \x20 return 1 + depth(n - 1)\n\
             end\n\
             depth(60)",
        );
    });
}

#[test]
fn nesting_that_outruns_the_stack_reports_rather_than_dying() {
    // The half of the limit a level *count* cannot express. A comparator
    // that sorts with itself nests forever, and each level costs native
    // stack — tens of kilobytes, several times more in a debug build than a
    // release one. Counted alone, 10 000 levels of it needs more stack than
    // a thread may have, and running off the end is a `SIGSEGV`: no span,
    // no message, no `catch`, and in the language server no session either.
    //
    // The budget is deliberately far below this thread's real stack, so the
    // guard is what stops the recursion and the test cannot depend on the
    // profile it was built in.
    const STACK: usize = 16 << 20;
    const BUDGET: usize = 2 << 20;
    std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(|| {
            saule_runtime::call::set_stack_budget(BUDGET);
            let src = "local data: table<integer, integer> = {2, 1}\n\
                       local fn forever(a: integer, b: integer) -> boolean\n\
                       \x20 Table.sort(data, forever)\n\
                       \x20 return a < b\n\
                       end\n\
                       Table.sort(data, forever)\n\
                       1";
            let module = front_end(src);
            match vm(&module, src) {
                Some(Outcome::Error(e)) => assert!(
                    e.contains("ran out of stack"),
                    "expected the stack guard to stop it, got: {e}"
                ),
                other => panic!("expected an error, got {other:?}"),
            }
        })
        .expect("spawn")
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e));
}
