//! Calls, lambdas, closures, and tail calls (`VM_DESIGN.md` §6.4).

use crate::harness::*;

// ── functions and calls ───────────────────────────────────────────────────

#[test]
fn a_function_call_matches() {
    must_agree("fn double(n: integer) -> integer\n  return n * 2\nend\ndouble(21)");
    must_agree("fn add(a: integer, b: integer) -> integer\n  return a + b\nend\nadd(2, 3)");
    must_agree("fn zero() -> integer\n  return 0\nend\nzero()");
}

#[test]
fn a_forward_call_matches() {
    // `a` calls `b` before `b` is declared — the reason proto indices are
    // reserved in a pre-pass.
    must_agree(
        "fn a(n: integer) -> integer\n  return b(n) + 1\nend\n\
         fn b(n: integer) -> integer\n  return n * 10\nend\n\
         a(4)",
    );
}

#[test]
fn recursion_matches() {
    // The Phase 2 milestone: `fib` through `CALLK`, compiled from source.
    must_agree(
        "fn fib(n: integer) -> integer\n\
         \x20 if n < 2 then return n end\n\
         \x20 return fib(n - 1) + fib(n - 2)\n\
         end\n\
         fib(20)",
    );
    must_agree(
        "fn fact(n: integer) -> integer\n\
         \x20 if n <= 1 then return 1 end\n\
         \x20 return n * fact(n - 1)\n\
         end\n\
         fact(10)",
    );
}

#[test]
fn a_function_falling_off_the_end_returns_nil_in_both() {
    must_agree("fn nothing(n: integer) -> nil\n  local x: integer = n\nend\nnothing(1)");
}

#[test]
fn early_return_matches() {
    must_agree(
        "fn classify(n: integer) -> integer\n\
         \x20 if n < 0 then return -1 end\n\
         \x20 if n == 0 then return 0 end\n\
         \x20 return 1\n\
         end\n\
         classify(-5) * 100 + classify(0) * 10 + classify(9)",
    );
}

#[test]
fn a_function_used_as_a_value_matches() {
    // Declaring a `fn` also binds its name, so it is not only a `CALLK`
    // target — the tree-walker treats it as a value and so must the VM.
    must_agree("fn f() -> integer\n  return 1\nend\nlocal g = f\n2");
}

// ── lambdas and closures ──────────────────────────────────────────────────

#[test]
fn a_lambda_matches() {
    must_agree("local f = fn(n: integer) -> integer\n  return n * 2\nend\nf(21)");
    must_agree("local f = (n: integer) => n + 1\nf(41)");
}

#[test]
fn a_closure_reads_its_captured_variable() {
    must_agree(
        "fn make() -> integer\n\
         \x20 local base: integer = 40\n\
         \x20 local f = fn() -> integer\n\
         \x20   return base + 2\n\
         \x20 end\n\
         \x20 return f()\n\
         end\n\
         make()",
    );
}

#[test]
fn a_closure_writes_through_to_its_captured_variable() {
    // The live-binding half: `SETUPVAL`, not a copy.
    must_agree(
        "fn run() -> integer\n\
         \x20 local n: integer = 0\n\
         \x20 local bump = fn() -> nil\n\
         \x20   n = n + 1\n\
         \x20 end\n\
         \x20 bump()\n\
         \x20 bump()\n\
         \x20 bump()\n\
         \x20 return n\n\
         end\n\
         run()",
    );
}

#[test]
fn a_closure_sees_writes_made_after_it_was_built() {
    must_agree(
        "fn run() -> integer\n\
         \x20 local n: integer = 1\n\
         \x20 local read = fn() -> integer\n\
         \x20   return n\n\
         \x20 end\n\
         \x20 n = 41\n\
         \x20 return read() + 1\n\
         end\n\
         run()",
    );
}

#[test]
fn capture_threads_through_two_function_boundaries() {
    // The middle closure must gain an upvalue it never mentions, so the
    // inner one reaches through it rather than past it.
    must_agree(
        "fn outer() -> integer\n\
         \x20 local base: integer = 40\n\
         \x20 local mid = fn() -> integer\n\
         \x20   local inner = fn() -> integer\n\
         \x20     return base + 2\n\
         \x20   end\n\
         \x20   return inner()\n\
         \x20 end\n\
         \x20 return mid()\n\
         end\n\
         outer()",
    );
}

#[test]
fn a_lambda_capturing_nothing_captures_nothing() {
    must_agree("local f = fn() -> integer\n  return 7\nend\nf()");
}

#[test]
fn two_closures_share_one_captured_binding() {
    must_agree(
        "fn pair() -> integer\n\
         \x20 local n: integer = 0\n\
         \x20 local inc = fn() -> nil\n\
         \x20   n = n + 10\n\
         \x20 end\n\
         \x20 local dec = fn() -> nil\n\
         \x20   n = n - 1\n\
         \x20 end\n\
         \x20 inc()\n\
         \x20 inc()\n\
         \x20 dec()\n\
         \x20 return n\n\
         end\n\
         pair()",
    );
}

// ── per-iteration capture ─────────────────────────────────────────────────
//
// Each iteration's closure keeps the value *it* saw. The VM closes a loop's
// registers before the next iteration writes them; before that was done
// per loop rather than per body block, a numeric `for` never closed its own
// variable, and `continue` / `break` jumped past the body's close — so a
// closure read whatever reused the register next, a function in one case.

/// Collect one closure per iteration of `loop_head … end`, whose body is
/// `body`, then call them all. Every closure returns `captured`.
fn captures_in(loop_head: &str, prelude: &str, body: &str, captured: &str) -> String {
    format!(
        "fn build() -> string\n\
         \x20 local fns: table<fn() -> integer> = {{}}\n\
         \x20 {prelude}\n\
         \x20 {loop_head}\n\
         \x20   {body}\n\
         \x20   Table.insert(fns, fn() -> integer\n\
         \x20     return {captured}\n\
         \x20   end)\n\
         \x20 end\n\
         \x20 local out: string = \"\"\n\
         \x20 for _, f in fns do\n\
         \x20   out ..= tostring(f()) .. \",\"\n\
         \x20 end\n\
         \x20 return out\n\
         end\n\
         build()"
    )
}

#[test]
fn a_numeric_for_variable_is_captured_per_iteration() {
    must_agree(&captures_in("for i = 1, 3 do", "", "", "i"));
    must_agree(&captures_in(
        "for i = 1, 3 do",
        "",
        "local d: integer = i * 2",
        "d",
    ));
}

#[test]
fn a_for_in_variable_is_captured_per_iteration() {
    let src = captures_in(
        "for _, v in src do",
        "local src: table<integer> = {10, 20, 30}",
        "",
        "v",
    );
    must_agree(&src);
}

#[test]
fn a_while_body_local_is_captured_per_iteration() {
    must_agree(&captures_in(
        "while n < 3 do",
        "local n: integer = 0",
        "n += 1\n    local k: integer = n * 7",
        "k",
    ));
}

#[test]
fn continue_does_not_skip_closing_what_the_iteration_captured() {
    // The closure is built *before* the `continue`, so the body's own close
    // at its end is exactly what the jump skips.
    must_agree(
        "fn build() -> string\n\
         \x20 local fns: table<fn() -> integer> = {}\n\
         \x20 for i = 1, 4 do\n\
         \x20   local sq: integer = i * i\n\
         \x20   Table.insert(fns, fn() -> integer\n\
         \x20     return sq + i\n\
         \x20   end)\n\
         \x20   if i % 2 == 0 then\n\
         \x20     continue\n\
         \x20   end\n\
         \x20   local pad: integer = 0\n\
         \x20 end\n\
         \x20 local out: string = \"\"\n\
         \x20 for _, f in fns do\n\
         \x20   out ..= tostring(f()) .. \",\"\n\
         \x20 end\n\
         \x20 return out\n\
         end\n\
         build()",
    );
}

#[test]
fn break_does_not_leave_a_captured_loop_register_open() {
    // After `break`, the loop's registers go to the next statement. A
    // closure still pointing at one would read the new occupant.
    must_agree(
        "fn build() -> integer\n\
         \x20 local keep: fn() -> integer = fn() -> integer\n\
         \x20   return 0\n\
         \x20 end\n\
         \x20 for i = 5, 9 do\n\
         \x20   keep = fn() -> integer\n\
         \x20     return i\n\
         \x20   end\n\
         \x20   break\n\
         \x20 end\n\
         \x20 local a: integer = 100\n\
         \x20 local b: integer = 200\n\
         \x20 local c: integer = 300\n\
         \x20 local d: integer = 400\n\
         \x20 return keep() + a + b + c + d\n\
         end\n\
         build()",
    );
}

#[test]
fn a_repeat_body_local_is_captured_per_iteration() {
    must_agree(
        "fn build() -> string\n\
         \x20 local fns: table<fn() -> integer> = {}\n\
         \x20 local n: integer = 0\n\
         \x20 repeat\n\
         \x20   n += 1\n\
         \x20   local k: integer = n * 3\n\
         \x20   Table.insert(fns, fn() -> integer\n\
         \x20     return k\n\
         \x20   end)\n\
         \x20 until k >= 9\n\
         \x20 local out: string = \"\"\n\
         \x20 for _, f in fns do\n\
         \x20   out ..= tostring(f()) .. \",\"\n\
         \x20 end\n\
         \x20 return out\n\
         end\n\
         build()",
    );
}

#[test]
fn an_outer_local_captured_in_a_nested_block_is_closed_with_its_own_block() {
    // `x` belongs to the `if` block; the lambda capturing it sits one block
    // deeper. The `if` block's exit hands `x`'s register to `y`, so it is
    // that exit which has to close it — not the inner block's.
    must_agree(
        "fn build() -> integer\n\
         \x20 local f: fn() -> integer = fn() -> integer\n\
         \x20   return 0\n\
         \x20 end\n\
         \x20 if true then\n\
         \x20   local x: integer = 42\n\
         \x20   if true then\n\
         \x20     f = fn() -> integer\n\
         \x20       return x\n\
         \x20     end\n\
         \x20   end\n\
         \x20 end\n\
         \x20 if true then\n\
         \x20   local y: integer = 7\n\
         \x20   return f() + y\n\
         \x20 end\n\
         \x20 return -1\n\
         end\n\
         build()",
    );
}

#[test]
fn a_user_fn_named_main_keeps_its_locals_in_registers() {
    // The module body's proto is called `main`; the compiler used to tell
    // them apart by that name, so this function's `local`s compiled as
    // module slots and the module fell back to the tree-walker.
    let src = "local x: integer = 3\n\
               fn main() -> integer\n\
               \x20 local x: integer = 41\n\
               \x20 return x + 1\n\
               end\n\
               main() * 10 + x";
    must_agree(src);
}

#[test]
fn an_unreachable_nested_fn_declaration_compiles_to_nothing() {
    // The resolver binds no name for a `fn` declared inside a body, so it
    // cannot be called — `helper()` below resolves to the top-level one.
    // Declaring it is dead code, and it must not replace the top-level
    // function either, which the compiler finds by name.
    //
    // Pinned to the VM's answer, not agreement: the tree-walker bound the
    // nested `fn` by name at run time and answered 99, contradicting the
    // resolver that had already checked the call against the top-level
    // `helper`'s signature.
    let src = "fn helper() -> integer\n\
               \x20 return 1\n\
               end\n\
               fn outer() -> integer\n\
               \x20 fn helper() -> integer\n\
               \x20   return 99\n\
               \x20 end\n\
               \x20 return helper()\n\
               end\n\
               outer()";
    assert_eq!(
        vm(&front_end(src), src),
        Some(Outcome::Value("integer:1".into()))
    );
}

#[test]
fn a_trailing_block_to_a_native_binds_positionally() {
    // A block-bodied lambda as the last argument is a trailing block by
    // shape. A native has no declared parameter list to bind it against;
    // it is simply the next argument.
    must_agree(
        "local fns: table<fn() -> integer> = {}\n\
         Table.insert(fns, fn() -> integer\n\
         \x20 return 5\n\
         end)\n\
         fns[1]!()",
    );
}

// ── §6.4 tail calls ───────────────────────────────────────────────────────
//
// The two engines must agree about **which** calls are tail calls, not just
// that both have them: the depth at which a program dies is observable, and
// a mismatch either way is a divergence. The tree-walker's rule, in
// `Stmt::Return`, is the specification — a single returned expression that
// is a call, whose callee is not a `Member`/`SafeMember`, and which
// evaluates to a `Value::Function`. `exec_try` then forces one back into an
// ordinary call, and `run_in` does the same for a module body.
//
// Depth is what these assert, so each recursion is deeper than the 10 000
// the shared guard allows. A test that ran 100 levels would pass whether or
// not the tail call happened.

#[test]
fn a_tail_recursive_top_level_fn_runs_in_constant_depth() {
    must_agree(
        "fn countdown(n: integer, acc: integer) -> integer\n\
         \x20 if n == 0 then\n    return acc\n  end\n\
         \x20 return countdown(n - 1, acc + n)\n\
         end\n\
         countdown(50000, 0)",
    );
}

#[test]
fn a_tail_recursive_static_method_runs_in_constant_depth() {
    // `class Main` / `static fn` is the idiomatic shape of a Saule program,
    // so this is the commonest tail-recursive function in the language —
    // and it needs its own opcode, because a static method's proto is
    // reached through the class table rather than named directly.
    must_agree(
        "class Sum\n\
         \x20 static fn down(n: integer, acc: integer) -> integer\n\
         \x20   if n == 0 then\n      return acc\n    end\n\
         \x20   return down(n - 1, acc + n)\n\
         \x20 end\n\
         end\n\
         Sum.down(50000, 0)",
    );
}

#[test]
fn a_tail_recursive_lambda_runs_in_constant_depth() {
    // The callee is a *value* here, so whether this is a tail call is a
    // run-time question — `TAILCALL` asks it the same way the tree-walker's
    // `Value::Function` check does.
    must_agree(
        "local step: fn(integer, integer) -> integer = \
         fn(n: integer, acc: integer) -> integer\n\
         \x20 if n == 0 then\n    return acc\n  end\n\
         \x20 return step(n - 1, acc + n)\n\
         end\n\
         step(50000, 0)",
    );
}

#[test]
fn a_tail_call_inside_a_try_is_not_a_tail_call() {
    // **The direction that is easy to get wrong.** A handler has to still be
    // on the stack when the callee runs, or `try return f() catch` stops
    // catching what `f` throws — so `exec_try` forces the tail call into a
    // real one, and the compiler must not emit one inside a protected range.
    // Getting this wrong makes the VM survive where the tree-walker
    // overflows, which no output comparison on a terminating program can
    // see: it is a difference in depth, not in value.
    let src = "fn down(n: integer, acc: integer) -> integer\n\
               \x20 if n == 0 then\n    return acc\n  end\n\
               \x20 try\n\
               \x20   return down(n - 1, acc + n)\n\
               \x20 catch e: any\n\
               \x20   return -1\n\
               \x20 end\n\
               end\n\
               down(6, 0)";
    must_agree(src);
    assert!(
        !disasm_of(src).contains("TAILCALL"),
        "a `return` inside a `try` body must not compile to a tail call\n{}",
        disasm_of(src)
    );

    // The `catch` body is *outside* the protected range, and `exec_try`
    // forces only the body — so a tail call there is correct on both sides.
    let caught = "fn down(n: integer, acc: integer) -> integer\n\
                  \x20 if n == 0 then\n    return acc\n  end\n\
                  \x20 try\n\
                  \x20   throw \"again\"\n\
                  \x20 catch e: any\n\
                  \x20   return down(n - 1, acc + n)\n\
                  \x20 end\n\
                  end\n\
                  down(6, 0)";
    must_agree(caught);
    assert!(
        disasm_of(caught).contains("TAILCALL"),
        "a `return` in a `catch` body is past the handler and stays a tail \
         call\n{}",
        disasm_of(caught)
    );
}

#[test]
fn a_try_around_a_tail_call_still_catches_what_the_callee_throws() {
    // The reason for the rule above, checked directly rather than inferred:
    // if the frame were replaced, the handler would already be gone.
    must_agree(
        "fn boom() -> integer\n  throw \"bang\"\nend\n\
         fn guarded() -> string\n\
         \x20 try\n\
         \x20   return \"\" .. boom()\n\
         \x20 catch e: any\n\
         \x20   return \"caught \" .. tostring(e)\n\
         \x20 end\n\
         end\n\
         guarded()",
    );
    // And the same with the call *itself* in tail position.
    must_agree(
        "fn boom() -> integer\n  throw \"bang\"\nend\n\
         fn guarded() -> integer\n\
         \x20 try\n\
         \x20   return boom()\n\
         \x20 catch e: any\n\
         \x20   return -7\n\
         \x20 end\n\
         end\n\
         guarded()",
    );
}

#[test]
fn a_method_call_in_tail_position_is_not_a_tail_call() {
    // Deliberately excluded on both sides. `obj.m()` resolves through
    // `dispatch_member_call_multi`, which binds a receiver and handles
    // natives, enum variants and file handles; routing that through the
    // trampoline would mean reimplementing it — so the tree-walker's rule
    // names `Member`/`SafeMember` explicitly, and the compiler must agree.
    let src = "class Counter\n\
               \x20 fn down(n: integer, acc: integer) -> integer\n\
               \x20   if n == 0 then\n      return acc\n    end\n\
               \x20   return self.down(n - 1, acc + n)\n\
               \x20 end\n\
               end\n\
               local c: Counter = Counter()\n\
               c.down(6, 0)";
    must_agree(src);
    assert!(
        !disasm_of(src).contains("TAILCALL"),
        "a method call must not compile to a tail call\n{}",
        disasm_of(src)
    );
}

#[test]
fn a_tail_call_to_a_native_is_an_ordinary_call() {
    // A native has no Saule frame to replace, so `Flow::TailCall` is never
    // built for one and `TAILCALL` falls back to calling it here and
    // returning — including the multi-value case, which goes back through
    // `store_results` rather than `pop_frame`.
    must_agree(
        "fn upper(s: string) -> string\n  return String.upper(s)\nend\n\
         upper(\"abc\")",
    );
    must_agree(
        "fn find(h: string, n: string) -> (integer?, integer?)\n\
         \x20 return String.find(h, n)\n\
         end\n\
         local s: integer?, e: integer? = find(\"hello world\", \"world\")\n\
         local r: string = tostring(s) .. \"/\" .. tostring(e)\nr",
    );
}

#[test]
fn a_tail_call_to_a_constructor_is_an_ordinary_call() {
    // `ClassName(args)` evaluates to a `Value::Class`, not a
    // `Value::Function`, so the tree-walker makes it for real.
    must_agree(
        "class Box\n  v: integer\n  fn init(v: integer)\n    self.v = v\n  end\nend\n\
         fn make(v: integer) -> Box\n  return Box(v)\nend\n\
         make(4).v",
    );
}

#[test]
fn a_tail_call_still_passes_every_result_through() {
    // The frame is replaced but `ret_to` and `n_ret` are inherited, so
    // multi-return survives a tail chain without anything extra. Two
    // levels, so the results cross a replaced frame rather than just a
    // pushed one.
    must_agree(
        "fn pair() -> (integer, integer)\n  return 11, 22\nend\n\
         fn one() -> (integer, integer)\n  return pair()\nend\n\
         fn two() -> (integer, integer)\n  return one()\nend\n\
         local a: integer, b: integer = two()\n\
         local r: string = a .. \"/\" .. b\nr",
    );
}

#[test]
fn a_tail_call_closes_the_upvalues_of_the_frame_it_replaces() {
    // The replaced frame's registers are about to become the callee's
    // arguments, so a closure built in it must have stopped pointing at
    // them. This is `pop_frame`'s rule; a tail call ends a frame just as
    // surely as a return does, and skipping it would hand the closure
    // whatever the *next* iteration wrote.
    must_agree(
        "fn build(n: integer, acc: table) -> table\n\
         \x20 if n == 0 then\n    return acc\n  end\n\
         \x20 local captured: integer = n\n\
         \x20 acc[#acc + 1] = fn() -> integer return captured end\n\
         \x20 return build(n - 1, acc)\n\
         end\n\
         local fs: table = build(3, {})\n\
         local out: string = \"\"\n\
         for f in fs do\n  out = out .. f() .. \",\"\nend\n\
         out",
    );
}

#[test]
fn mutual_tail_recursion_runs_in_constant_depth() {
    // Two frames alternating, each replacing the other — the shape that
    // proves the frame is genuinely reused rather than merely reset.
    must_agree(
        "fn even(n: integer) -> boolean\n\
         \x20 if n == 0 then\n    return true\n  end\n\
         \x20 return odd(n - 1)\n\
         end\n\
         fn odd(n: integer) -> boolean\n\
         \x20 if n == 0 then\n    return false\n  end\n\
         \x20 return even(n - 1)\n\
         end\n\
         even(50000)",
    );
}

#[test]
fn a_tail_call_with_defaulted_and_variadic_parameters_binds_them() {
    // The frame is entered through `entry_for(n_args)` exactly as a pushed
    // one is, so a default still runs in the callee and `VARARG` still
    // gathers — but the frame it enters is *dirty*, holding the previous
    // call's registers rather than fresh stack, which is what makes the
    // missing-parameter fill load-bearing here in a way it is not for a
    // pushed frame.
    must_agree(
        "fn walk(n: integer, acc: integer = 100) -> integer\n\
         \x20 if n == 0 then\n    return acc\n  end\n\
         \x20 return walk(n - 1, acc + n)\n\
         end\n\
         walk(20000)",
    );
    must_agree(
        "fn gather(n: integer, ...rest: integer) -> integer\n\
         \x20 if n == 0 then\n    return #rest\n  end\n\
         \x20 return gather(n - 1, 1, 2, 3)\n\
         end\n\
         gather(20000)",
    );
}

#[test]
fn a_lambda_in_a_method_body_reaches_self() {
    // `self` is an ordinary local of the enclosing frame -- `method_proto`
    // declares it at register 0 under that name -- so the capture walk every
    // other free variable takes reaches it. It used to refuse as ``self`
    // outside a method`, because the test was `in_method` on the *lambda's*
    // frame, which is never a method.
    must_agree(
        "class Widget\n           label: string\n           fn init(label: string)\n             self.label = label\n           end\n           fn describe() -> string\n             local f: fn() -> string = fn()\n               return self.label\n             end\n             return f()\n           end\n         end\n         Widget(\"w\").describe()",
    );
}

#[test]
fn a_self_recursive_local_lambda_does_not_capture_itself() {
    // `SELFFUNC`: the name is not a capturable local of the enclosing frame
    // (its register is declared *after* the initializer compiles), and
    // capturing it once it were would close an `Rc` cycle per call -- the
    // leak the tree-walker's `FunctionObject::self_name` exists to avoid.
    must_agree(
        "fn fact(n: integer) -> integer\n           local go: fn(integer) -> integer = fn(k: integer)\n             if k <= 1 then\n               return 1\n             end\n             return k * go(k - 1)\n           end\n           return go(n)\n         end\n         fact(6)",
    );
}

#[test]
fn calling_a_non_callable_value_fails_the_same_way() {
    // `CALL` and the tree-walker's `call_value_multi` compile the same
    // source, and they used to word this differently — "attempt to call a
    // `integer`" against "value of type `integer` is not callable". Nothing
    // caught it because every program that reached the VM's version was
    // refused by the compiler for some other reason first; lifting the
    // refusal on a function-valued field call is what exposed it. Both now
    // go through `RuntimeError::not_callable`.
    must_agree("local f: any = 5\nf()");
    must_agree("local s: any = \"x\"\ns()");
}
