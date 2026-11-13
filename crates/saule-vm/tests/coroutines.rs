//! Coroutines: suspending a function part-way and resuming it later.
//!
//! These drive coroutines through `any`-typed handles, so what they pin is
//! the *runtime*: ordering, values in both directions, the call stack a
//! suspended coroutine keeps, captured locals across suspension, status,
//! failure, misuse, and memory. The typed surface — `Coroutine<T>`,
//! `Yielder<T>`, inference, `for … in` — is `coroutines_typed.rs`.
//!
//! Most bodies here fit the typed contract `fn(Yielder<T>) -> nil` and are
//! passed to `Coroutine.create` directly. The tests about what that contract
//! leaves out — arguments to the first `resume`, values passed back in
//! through `resume(x)`, a body's return values, several values at once —
//! hold the body as `any` first (`local body: any = fn … end`), which is the
//! dynamic escape hatch: an `any` body is accepted, and the runtime does
//! everything Lua's coroutines do.
//!
//! Captured locals are declared inside a function wherever the test is
//! about upvalues: a top-level local is a module slot, not a register, and
//! would pass whether or not upvalues worked.

use std::rc::Rc;

use saule_lexer::Lexer;
use saule_parser::parse;
use saule_runtime::Value;

fn run(src: &str) -> Result<Value, saule_vm::PipelineError> {
    let toks = Lexer::new(src).tokenize().expect("lex");
    let mut module = parse(toks).expect("parse");
    saule_vm::check_and_run(&mut module, "test.sau", src)
}

fn eval(src: &str) -> Value {
    match run(src) {
        Ok(v) => v,
        Err(e) => panic!("pipeline failed: {e:?}\n--- source ---\n{src}"),
    }
}

fn text(src: &str) -> String {
    match eval(src) {
        Value::Str(s) => (*s).clone(),
        other => panic!("expected a string, got {other:?}\n--- source ---\n{src}"),
    }
}

/// The program must fail at run time; returns the error, rendered.
fn fails(src: &str) -> String {
    match run(src) {
        Ok(v) => panic!("expected a runtime failure, got {v:?}\n--- source ---\n{src}"),
        Err(saule_vm::PipelineError::Runtime(e)) => format!("{e}"),
        Err(e) => panic!("expected a runtime failure, got {e:?}\n--- source ---\n{src}"),
    }
}

/// Run `f` on a thread with a large stack, all of it budgeted the way
/// `saule run` budgets its own — for tests that nest resumes deeply enough
/// that the guard against running out of native stack is what stops them.
/// Each nested resume is a dispatch loop under its own Rust frames, and a
/// debug build's frames are several times a release build's.
fn on_a_real_stack(f: impl FnOnce() + Send + 'static) {
    const STACK: usize = 64 << 20;
    std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(move || {
            saule_runtime::call::set_stack_budget(STACK);
            f()
        })
        .expect("spawn")
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e));
}

// ── Basics ───────────────────────────────────────────────────────────────

#[test]
fn a_new_coroutine_does_not_run_until_resumed() {
    assert_eq!(
        text(
            r#"
fn go() -> string
  local ran: integer = 0
  local gen: any = Coroutine.create(fn(co: any) -> nil
    ran = ran + 1
  end)
  local before: string = tostring(gen.status()) .. ran
  gen.resume()
  return before .. "/" .. tostring(gen.status()) .. ran
end
go()
"#
        ),
        "suspended0/dead1"
    );
}

#[test]
fn the_first_resume_passes_its_arguments_to_the_body() {
    // After the yielder, which is always the body's first parameter.
    assert_eq!(
        text(
            r#"
fn go() -> string
  local body: any = fn(co: any, a: integer, b: string) -> string
    return b .. a
  end
  local gen: any = Coroutine.create(body)
  return tostring(gen.resume(7, "x"))
end
go()
"#
        ),
        "x7"
    );
}

#[test]
fn yields_come_out_in_order_and_a_finished_coroutine_gives_nil() {
    assert_eq!(
        text(
            r#"
fn go() -> string
  local gen: any = Coroutine.create(fn(co: any) -> nil
    for i = 1, 3 do
      co.yield(i * 10)
    end
  end)
  local log: string = ""
  for i = 1, 5 do
    log = log .. tostring(gen.resume()) .. ","
  end
  return log .. tostring(gen.status())
end
go()
"#
        ),
        "10,20,30,nil,nil,dead"
    );
}

#[test]
fn what_resume_is_passed_is_what_yield_returns() {
    assert_eq!(
        text(
            r#"
fn go() -> string
  local body: any = fn(co: any, first: string) -> string
    local log: string = first
    local got: any = co.yield("y1")
    log = log .. tostring(got)
    got = co.yield("y2")
    log = log .. tostring(got)
    return log
  end
  local gen: any = Coroutine.create(body)
  local a: string = tostring(gen.resume("a"))
  local b: string = tostring(gen.resume("b"))
  local c: string = tostring(gen.resume("c"))
  return a .. "," .. b .. "," .. c
end
go()
"#
        ),
        "y1,y2,abc"
    );
}

#[test]
fn several_values_pass_each_way() {
    assert_eq!(
        text(
            r#"
fn go() -> string
  local body: any = fn(co: any) -> nil
    local x: any?, y: any? = co.yield(1, "two")
    co.yield(tostring(x) .. tostring(y))
  end
  local gen: any = Coroutine.create(body)
  local a: any?, b: any? = gen.resume()
  local c: any = gen.resume(3, "four")
  return tostring(a) .. "," .. tostring(b) .. "," .. tostring(c)
end
go()
"#
        ),
        "1,two,3four"
    );
}

#[test]
fn a_yields_result_lands_wherever_the_call_put_it() {
    // Discarded, passed straight on as an argument, and returned directly:
    // three different result shapes the resumed `yield` call has to honour.
    //
    // As an argument it is one value — `1+nil`, not `1+2` — because the
    // checker types a dynamic call as a single `any` and the call to `join`
    // is bound at compile time to that one argument. Returned directly it is
    // *every* value (`xy`): the return takes the call's results whole.
    assert_eq!(
        text(
            r#"
fn join(a: any?, b: any? = nil) -> string
  return tostring(a) .. "+" .. tostring(b)
end
fn go() -> string
  local body: any = fn(co: any) -> any
    co.yield("discard")
    co.yield(join(co.yield("args")))
    return co.yield("tail")
  end
  local gen: any = Coroutine.create(body)
  local log: string = tostring(gen.resume()) .. ","
  log = log .. tostring(gen.resume("ignored", "too")) .. ","
  log = log .. tostring(gen.resume(1, 2)) .. ","
  log = log .. tostring(gen.resume()) .. ","
  local a: any?, b: any? = gen.resume("x", "y")
  return log .. tostring(a) .. tostring(b) .. "," .. tostring(gen.status())
end
go()
"#
        ),
        "discard,args,1+nil,tail,xy,dead"
    );
}

#[test]
fn a_bare_yield_resumes_the_caller_with_nil() {
    assert_eq!(
        text(
            r#"
fn go() -> string
  local body: any = fn(co: any) -> string
    local got: any? = co.yield()
    return "got " .. tostring(got)
  end
  local gen: any = Coroutine.create(body)
  local first: any? = gen.resume()
  local second: any? = gen.resume()
  return tostring(first) .. "/" .. tostring(second)
end
go()
"#
        ),
        "nil/got nil"
    );
}

#[test]
fn the_body_returns_its_values_through_the_last_resume() {
    assert_eq!(
        text(
            r#"
fn go() -> string
  local body: any = fn(co: any) -> (integer, string)
    co.yield(0)
    return 1, "end"
  end
  local gen: any = Coroutine.create(body)
  gen.resume()
  local a: any?, b: any? = gen.resume()
  return tostring(a) .. tostring(b) .. tostring(gen.isDone())
end
go()
"#
        ),
        "1endtrue"
    );
}

#[test]
fn a_dead_coroutine_keeps_answering_nil() {
    assert_eq!(
        text(
            r#"
fn go() -> string
  local body: any = fn(co: any) -> string
    return "only"
  end
  local gen: any = Coroutine.create(body)
  local log: string = ""
  for i = 1, 4 do
    log = log .. tostring(gen.resume()) .. ","
  end
  return log .. tostring(gen.isDone())
end
go()
"#
        ),
        "only,nil,nil,nil,true"
    );
}

#[test]
fn a_named_function_can_be_the_body() {
    assert_eq!(
        text(
            r#"
fn producer(co: any, n: integer)
  for i = 1, n do
    co.yield(i)
  end
end
fn go() -> string
  local gen: any = Coroutine.create(producer)
  local log: string = tostring(gen.resume(2))
  log = log .. tostring(gen.resume())
  log = log .. tostring(gen.resume())
  return log .. tostring(gen.status())
end
go()
"#
        ),
        "12nildead"
    );
}

#[test]
fn a_coroutine_works_at_the_top_level() {
    // Top-level locals are module slots, so this is the path that does not
    // involve upvalues at all.
    assert_eq!(
        text(
            r#"
local total: integer = 0
local gen: any = Coroutine.create(fn(co: any) -> nil
  for i = 1, 4 do
    total = total + i
    co.yield(total)
  end
end)
local log: string = ""
while gen.isDone() == false do
  log = log .. tostring(gen.resume()) .. ";"
end
log .. total
"#
        ),
        "1;3;6;10;nil;10"
    );
}

// ── Stackful: yielding from anywhere in the coroutine's own call stack ──

#[test]
fn a_function_the_body_calls_can_yield() {
    assert_eq!(
        text(
            r#"
fn emit(co: any, n: integer)
  co.yield(n)
  co.yield(n + 1)
end
fn go() -> string
  local gen: any = Coroutine.create(fn(co: any) -> nil
    emit(co, 1)
    emit(co, 10)
  end)
  local log: string = ""
  for i = 1, 5 do
    log = log .. tostring(gen.resume()) .. ","
  end
  return log
end
go()
"#
        ),
        "1,2,10,11,nil,"
    );
}

#[test]
fn yielding_after_a_recursive_call_returns_resumes_in_the_right_frame() {
    // Post-order: each level yields only after the deeper ones returned, so
    // every resume continues a different frame of a stack that unwinds one
    // level per yield.
    assert_eq!(
        text(
            r#"
fn rec(co: any, n: integer)
  if n == 0 then
    return
  end
  rec(co, n - 1)
  co.yield(n)
end
fn go() -> string
  local gen: any = Coroutine.create(fn(co: any) -> nil
    rec(co, 5)
  end)
  local log: string = ""
  while gen.isDone() == false do
    log = log .. tostring(gen.resume()) .. ","
  end
  return log
end
go()
"#
        ),
        "1,2,3,4,5,nil,"
    );
}

#[test]
fn a_yield_hundreds_of_frames_deep() {
    assert_eq!(
        text(
            r#"
fn down(co: any, n: integer)
  if n == 0 then
    return
  end
  co.yield(n)
  down(co, n - 1)
end
fn go() -> string
  local gen: any = Coroutine.create(fn(co: any) -> nil
    down(co, 500)
  end)
  local sum: integer = 0
  local count: integer = 0
  local v: any? = gen.resume()
  while v != nil do
    count = count + 1
    v = gen.resume()
  end
  return count .. ":" .. tostring(gen.status())
end
go()
"#
        ),
        "500:dead"
    );
}

#[test]
fn a_method_can_yield() {
    assert_eq!(
        text(
            r#"
class Ticker
  local n: integer
  fn init()
    self.n = 0
  end
  fn run(co: any)
    while self.n < 3 do
      self.n = self.n + 1
      co.yield(self.n)
    end
  end
  fn count() -> integer
    return self.n
  end
end
fn go() -> string
  local t: Ticker = Ticker()
  local gen: any = Coroutine.create(fn(co: any) -> nil
    t.run(co)
  end)
  local log: string = ""
  for i = 1, 4 do
    log = log .. tostring(gen.resume()) .. ","
  end
  return log .. t.count()
end
go()
"#
        ),
        "1,2,3,nil,3"
    );
}

#[test]
fn a_tail_called_function_can_yield_and_its_result_ends_the_coroutine() {
    assert_eq!(
        text(
            r#"
fn helper(co: any) -> string
  co.yield("y")
  return "r"
end
fn go() -> string
  local body: any = fn(co: any) -> string
    return helper(co)
  end
  local gen: any = Coroutine.create(body)
  return tostring(gen.resume()) .. tostring(gen.resume()) .. tostring(gen.status())
end
go()
"#
        ),
        "yrdead"
    );
}

#[test]
fn a_yield_inside_try_resumes_inside_it_and_its_catch_still_works() {
    assert_eq!(
        text(
            r#"
fn go() -> string
  local body: any = fn(co: any) -> string
    try
      co.yield("in try")
      throw "after resume"
    catch e: string
      co.yield("caught " .. e)
    end
    return "end"
  end
  local gen: any = Coroutine.create(body)
  local log: string = ""
  for i = 1, 3 do
    log = log .. tostring(gen.resume()) .. ","
  end
  return log .. tostring(gen.error())
end
go()
"#
        ),
        "in try,caught after resume,end,nil"
    );
}

#[test]
fn a_yield_inside_for_in_keeps_the_iteration_state() {
    assert_eq!(
        text(
            r#"
fn go() -> string
  local items: table<string> = {"a", "b", "c"}
  local gen: any = Coroutine.create(fn(co: any) -> nil
    for i: integer, v: string in items do
      co.yield(i .. v)
    end
  end)
  local log: string = ""
  for k = 1, 4 do
    log = log .. tostring(gen.resume()) .. ","
  end
  return log
end
go()
"#
        ),
        "1a,2b,3c,nil,"
    );
}

#[test]
fn a_yield_inside_a_numeric_for_keeps_the_counter() {
    assert_eq!(
        text(
            r#"
fn go() -> string
  local gen: any = Coroutine.create(fn(co: any) -> nil
    for i = 10, 1, -3 do
      co.yield(i)
    end
  end)
  local log: string = ""
  while gen.isDone() == false do
    log = log .. tostring(gen.resume()) .. " "
  end
  return log
end
go()
"#
        ),
        "10 7 4 1 nil "
    );
}

// ── Locals: the coroutine's own, and the ones it shares ─────────────────

#[test]
fn the_bodys_locals_survive_every_suspension() {
    assert_eq!(
        text(
            r#"
fn go() -> string
  local body: any = fn(co: any) -> nil
    local sum: integer = 0
    local label: string = "s"
    while true do
      local got: any = co.yield(label .. sum)
      sum = sum + 1
      label = label .. "+"
    end
  end
  local gen: any = Coroutine.create(body)
  local log: string = ""
  for i = 1, 4 do
    log = log .. tostring(gen.resume()) .. ","
  end
  return log
end
go()
"#
        ),
        "s0,s+1,s++2,s+++3,"
    );
}

#[test]
fn a_captured_local_is_shared_both_ways_across_suspension() {
    // The coroutine writes the caller's local; the caller writes it back
    // while the coroutine is suspended; the coroutine sees that write.
    assert_eq!(
        text(
            r#"
fn go() -> string
  local shared: integer = 0
  local gen: any = Coroutine.create(fn(co: any) -> nil
    while true do
      shared = shared + 1
      co.yield(shared)
    end
  end)
  local log: string = tostring(gen.resume()) .. ","
  shared = shared + 10
  log = log .. tostring(gen.resume()) .. ","
  return log .. shared
end
go()
"#
        ),
        "1,12,12"
    );
}

#[test]
fn a_closure_over_a_suspended_coroutines_local_reads_and_writes_it() {
    // `mine` is a register in the suspended coroutine's own register file.
    // Read and written from the caller's, it has to go through its cell —
    // and the coroutine has to see the write when it resumes.
    assert_eq!(
        text(
            r#"
fn go() -> string
  local peek: fn() -> integer = fn() -> integer
    return -1
  end
  local poke: fn(integer) -> nil = fn(v: integer) -> nil
    return nil
  end
  local gen: any = Coroutine.create(fn(co: any) -> nil
    local mine: integer = 1
    peek = fn() -> integer
      return mine
    end
    poke = fn(v: integer) -> nil
      mine = v
    end
    co.yield(mine)
    mine = mine + 1
    co.yield(mine)
  end)
  local log: string = tostring(gen.resume()) .. ","
  log = log .. peek() .. ","
  poke(41)
  log = log .. peek() .. ","
  log = log .. tostring(gen.resume()) .. ","
  log = log .. peek()
  return log
end
go()
"#
        ),
        "1,1,41,42,42"
    );
}

#[test]
fn a_closure_outlives_the_coroutine_that_built_it() {
    assert_eq!(
        text(
            r#"
fn go() -> string
  local peek: fn() -> integer = fn() -> integer
    return -1
  end
  local gen: any = Coroutine.create(fn(co: any) -> nil
    local mine: integer = 7
    peek = fn() -> integer
      return mine
    end
    co.yield(nil)
    mine = 8
  end)
  gen.resume()
  gen.resume()
  local x: integer = 100
  local y: integer = 200
  return tostring(gen.status()) .. peek()
end
go()
"#
        ),
        "dead8"
    );
}

#[test]
fn two_coroutines_interleave_over_one_captured_local() {
    assert_eq!(
        text(
            r#"
fn go() -> string
  local log: string = ""
  local a: any = Coroutine.create(fn(co: any) -> nil
    for i = 1, 3 do
      log = log .. "a" .. i
      co.yield(nil)
    end
  end)
  local b: any = Coroutine.create(fn(co: any) -> nil
    for i = 1, 3 do
      log = log .. "b" .. i
      co.yield(nil)
    end
  end)
  for i = 1, 3 do
    a.resume()
    b.resume()
  end
  return log
end
go()
"#
        ),
        "a1b1a2b2a3b3"
    );
}

#[test]
fn the_callers_locals_survive_a_resume_that_runs_lots_of_code() {
    // The caller's open upvalues are parked for the whole resume. Lots of
    // work in the coroutine — including closures of its own — must not
    // disturb them.
    assert_eq!(
        text(
            r#"
fn go() -> string
  local mine: integer = 5
  local bump = fn() -> nil
    mine = mine + 1
  end
  local gen: any = Coroutine.create(fn(co: any) -> nil
    local acc: integer = 0
    for i = 1, 100 do
      local f = fn() -> integer
        return i * 2
      end
      acc = acc + f()
    end
    co.yield(acc)
  end)
  local r: any = gen.resume()
  bump()
  return tostring(r) .. ":" .. mine
end
go()
"#
        ),
        "10100:6"
    );
}

#[test]
fn a_nested_coroutine_shares_a_local_of_the_coroutine_around_it() {
    // `depth` is a register in the *outer* coroutine's file. While the
    // inner one runs, the outer is parked by its own `resume` call; while
    // both are suspended, both are parked. Writes from every side must
    // reach the one binding.
    assert_eq!(
        text(
            r#"
fn go() -> string
  local outer: any = Coroutine.create(fn(co: any) -> nil
    local depth: integer = 1
    local inner: any = Coroutine.create(fn(ico: any) -> nil
      while true do
        depth = depth * 10
        ico.yield(depth)
      end
    end)
    while true do
      local seen: any = inner.resume()
      depth = depth + 1
      co.yield(tostring(seen) .. ">" .. depth)
    end
  end)
  local log: string = ""
  for i = 1, 3 do
    log = log .. tostring(outer.resume()) .. ","
  end
  return log
end
go()
"#
        ),
        "10>11,110>111,1110>1111,"
    );
}

// ── Status and nesting ───────────────────────────────────────────────────

#[test]
fn status_is_running_inside_and_normal_while_waiting_on_another() {
    assert_eq!(
        text(
            r#"
fn go() -> string
  local holder: table<any> = {}
  local outer: any = Coroutine.create(fn(co: any) -> nil
    local me: any = holder[1]
    local inner: any = Coroutine.create(fn(ico: any) -> nil
      ico.yield(tostring(me.status()))
    end)
    local seenByInner: string = tostring(inner.resume())
    co.yield(tostring(me.status()) .. "/" .. seenByInner .. "/" .. tostring(inner.status()))
  end)
  holder[1] = outer
  local seen: string = tostring(outer.resume())
  return seen .. "/" .. tostring(outer.status())
end
go()
"#
        ),
        "running/normal/suspended/suspended"
    );
}

#[test]
fn a_coroutine_resumes_another_and_relays_its_values() {
    assert_eq!(
        text(
            r#"
fn go() -> string
  local log: string = ""
  local inner: any = Coroutine.create(fn(co: any) -> nil
    for i = 1, 3 do
      log = log .. "i" .. i
      co.yield(i)
    end
  end)
  local outer: any = Coroutine.create(fn(co: any) -> nil
    local v: any? = inner.resume()
    while v != nil do
      log = log .. "o"
      co.yield(tostring(v) .. tostring(v))
      v = inner.resume()
    end
  end)
  local got: string = ""
  local v: any? = outer.resume()
  while v != nil do
    got = got .. tostring(v) .. ","
    v = outer.resume()
  end
  return got .. log
end
go()
"#
        ),
        "11,22,33,i1oi2oi3o"
    );
}

#[test]
fn generators_compose_into_a_pipeline() {
    assert_eq!(
        text(
            r#"
fn range(n: integer) -> any
  return Coroutine.create(fn(co: any) -> nil
    for i = 1, n do
      co.yield(i)
    end
  end)
end
fn evens(src: any) -> any
  return Coroutine.create(fn(co: any) -> nil
    local v: any? = src.resume()
    while v != nil do
      if (v as integer)! % 2 == 0 then
        co.yield(v)
      end
      v = src.resume()
    end
  end)
end
fn squares(src: any) -> any
  return Coroutine.create(fn(co: any) -> nil
    local v: any? = src.resume()
    while v != nil do
      local n: integer = (v as integer)!
      co.yield(n * n)
      v = src.resume()
    end
  end)
end
fn go() -> string
  local p: any = squares(evens(range(10)))
  local log: string = ""
  local v: any? = p.resume()
  while v != nil do
    log = log .. tostring(v) .. ","
    v = p.resume()
  end
  return log
end
go()
"#
        ),
        "4,16,36,64,100,"
    );
}

#[test]
fn a_round_robin_scheduler_runs_many_coroutines_to_completion() {
    assert_eq!(
        text(
            r#"
fn go() -> string
  local tasks: table<any> = {}
  local steps: integer = 0
  for t = 1, 50 do
    tasks[t] = Coroutine.create(fn(co: any) -> nil
      for s = 1, 3 do
        steps = steps + 1
        co.yield(nil)
      end
    end)
  end
  local rounds: integer = 0
  local alive: boolean = true
  while alive do
    alive = false
    rounds = rounds + 1
    for t = 1, #tasks do
      local task: any = tasks[t]
      if task.isDone() == false then
        task.resume()
        alive = true
      end
    end
  end
  return steps .. "/" .. rounds
end
go()
"#
        ),
        // Three yields each, plus the resume that lets each one finish, plus
        // the final round that finds nothing alive.
        "150/5"
    );
}

#[test]
fn a_coroutine_can_be_resumed_from_inside_a_callback() {
    // Resuming is an ordinary call, so it is fine from a sort comparator:
    // the coroutine runs on its own register file and yields from its own
    // code, which is all a yield needs.
    assert_eq!(
        text(
            r#"
fn go() -> string
  local counter: any = Coroutine.create(fn(co: any) -> nil
    local n: integer = 0
    while true do
      n = n + 1
      co.yield(n)
    end
  end)
  local last: any? = nil
  local t: table<integer> = {3, 1, 2}
  Table.sort(t, fn(a: integer, b: integer) -> boolean
    last = counter.resume()
    return a < b
  end)
  return t[1] .. t[2] .. t[3] .. ":" .. tostring(last != nil) .. tostring(counter.status())
end
go()
"#
        ),
        "123:truesuspended"
    );
}

#[test]
fn a_coroutine_started_in_one_coroutine_can_be_resumed_from_anywhere() {
    assert_eq!(
        text(
            r#"
fn go() -> string
  local made: table<any> = {}
  local maker: any = Coroutine.create(fn(co: any) -> nil
    local child: any = Coroutine.create(fn(cc: any) -> nil
      cc.yield("first")
      cc.yield("second")
    end)
    made[1] = child
    co.yield(child.resume())
  end)
  local a: string = tostring(maker.resume())
  local child: any = made[1]
  local b: string = tostring(child.resume())
  return a .. "," .. b .. "," .. tostring(maker.status()) .. "," .. tostring(child.status())
end
go()
"#
        ),
        "first,second,suspended,suspended"
    );
}

// ── Failure: reported to `resume`, not raised ────────────────────────────

#[test]
fn a_throw_in_the_body_is_reported_and_the_coroutine_is_dead() {
    assert_eq!(
        text(
            r#"
fn go() -> string
  local gen: any = Coroutine.create(fn(co: any) -> nil
    co.yield("before")
    throw "boom"
  end)
  local log: string = tostring(gen.resume()) .. ","
  log = log .. tostring(gen.error()) .. ","
  log = log .. tostring(gen.resume()) .. ","
  log = log .. tostring(gen.status()) .. "," .. tostring(gen.isDone()) .. ","
  log = log .. tostring(gen.error()) .. ","
  log = log .. tostring(gen.resume()) .. ","
  return log .. tostring(gen.error())
end
go()
"#
        ),
        "before,nil,nil,dead,true,boom,nil,boom"
    );
}

#[test]
fn a_runtime_error_in_the_body_is_reported_with_its_message() {
    let out = text(
        r#"
fn go() -> string
  local gen: any = Coroutine.create(fn(co: any) -> nil
    local s: string? = nil
    local u: string = s!
  end)
  local r: any? = gen.resume()
  return tostring(r) .. "|" .. tostring(gen.status()) .. "|" .. tostring(gen.error())
end
go()
"#,
    );
    assert!(
        out.starts_with("nil|dead|") && out.contains("nil"),
        "got: {out}"
    );
    assert!(out.contains("force-unwrapped"), "got: {out}");
}

#[test]
fn a_failing_native_in_the_body_is_reported() {
    let out = text(
        r#"
fn go() -> string
  local gen: any = Coroutine.create(fn(co: any) -> nil
    error("custom failure")
  end)
  gen.resume()
  return tostring(gen.error())
end
go()
"#,
    );
    assert!(out.contains("custom failure"), "got: {out}");
}

#[test]
fn a_throw_caught_inside_the_coroutine_is_not_a_failure() {
    assert_eq!(
        text(
            r#"
fn go() -> string
  local body: any = fn(co: any) -> string
    try
      throw "handled"
    catch e: string
      co.yield(e)
    end
    return "fine"
  end
  local gen: any = Coroutine.create(body)
  return tostring(gen.resume()) .. tostring(gen.resume()) .. tostring(gen.error())
end
go()
"#
        ),
        "handledfinenil"
    );
}

#[test]
fn a_failed_coroutine_leaves_the_callers_locals_consistent() {
    // The step-1 case that could not be written without coroutines: the
    // body writes a captured local of the caller, builds a closure over a
    // local of its own that escapes, and then fails. The caller must see
    // the write, its own upvalues must be open again, and the escaped
    // closure must keep the value its register held.
    assert_eq!(
        text(
            r#"
fn go() -> string
  local n: integer = 0
  local read: fn() -> integer = fn() -> integer
    return n
  end
  local escaped: fn() -> integer = fn() -> integer
    return 0
  end
  local gen: any = Coroutine.create(fn(co: any) -> nil
    local mine: integer = 42
    escaped = fn() -> integer
      return mine + n
    end
    n = 5
    throw "stop"
  end)
  local r: any? = gen.resume()
  n = n + 1
  local x: integer = 7
  local y: integer = 9
  return tostring(r) .. "|" .. n .. "|" .. read() .. "|" .. escaped() .. "|" .. tostring(gen.error())
end
go()
"#
        ),
        "nil|6|6|48|stop"
    );
}

#[test]
fn a_closure_escaping_a_failed_callback_keeps_its_value() {
    // The other step-1 leftover: the failure happens in a sort comparator —
    // a register file of its own, discarded when the error leaves it — and
    // the coroutine around it is what lets the program carry on and look.
    let out = text(
        r#"
fn go() -> string
  local escaped: fn() -> integer = fn() -> integer
    return 0
  end
  local gen: any = Coroutine.create(fn(co: any) -> nil
    local t: table<integer> = {2, 1}
    Table.sort(t, fn(a: integer, b: integer) -> boolean
      local mine: integer = 42
      escaped = fn() -> integer
        return mine
      end
      throw "stop"
    end)
  end)
  gen.resume()
  local x: integer = 7
  local y: integer = 8
  return escaped() .. "|" .. tostring(gen.status()) .. "|" .. tostring(gen.error())
end
go()
"#,
    );
    assert!(out.starts_with("42|dead|"), "got: {out}");
    assert!(out.contains("stop"), "got: {out}");
}

#[test]
fn unbounded_recursion_in_the_body_is_reported_not_fatal() {
    let out = text(
        r#"
fn down(n: integer) -> integer
  return 1 + down(n + 1)
end
fn go() -> string
  local gen: any = Coroutine.create(fn(co: any) -> nil
    down(0)
  end)
  local r: any? = gen.resume()
  return tostring(r) .. "|" .. tostring(gen.error())
end
go()
"#,
    );
    assert!(out.starts_with("nil|"), "got: {out}");
    assert!(out.contains("stack overflow"), "got: {out}");
}

#[test]
fn a_failure_does_not_leave_a_stale_stack_trace_behind() {
    // The failure is handled by being reported. Its call stack must not be
    // left recorded, or the next unrelated error would be shown with it.
    text(
        r#"
fn go() -> string
  local gen: any = Coroutine.create(fn(co: any) -> nil
    throw "handled by resume"
  end)
  gen.resume()
  return "ok"
end
go()
"#,
    );
    let trace = saule_vm::take_stack_trace();
    assert!(
        trace.frames.is_empty(),
        "a reported failure left a trace: {trace:?}"
    );
}

// ── Misuse ───────────────────────────────────────────────────────────────

#[test]
fn a_coroutine_cannot_resume_itself() {
    // Raised inside the coroutine, so it is the coroutine that fails — and
    // that is reported to whoever resumed it.
    let out = text(
        r#"
fn go() -> string
  local holder: table<any> = {}
  local gen: any = Coroutine.create(fn(co: any) -> nil
    local me: any = holder[1]
    me.resume()
  end)
  holder[1] = gen
  gen.resume()
  return tostring(gen.status()) .. "|" .. tostring(gen.error())
end
go()
"#,
    );
    assert!(out.starts_with("dead|"), "got: {out}");
    assert!(out.contains("from inside itself"), "got: {out}");
}

#[test]
fn a_coroutine_waiting_on_another_cannot_be_resumed_by_it() {
    let out = text(
        r#"
fn go() -> string
  local holder: table<any> = {}
  local a: any = Coroutine.create(fn(co: any) -> nil
    local b: any = holder[2]
    local r: any? = b.resume()
    co.yield("b gave " .. tostring(r) .. " and failed with: " .. tostring(b.error()))
  end)
  local b: any = Coroutine.create(fn(co: any) -> nil
    local a2: any = holder[1]
    a2.resume()
  end)
  holder[1] = a
  holder[2] = b
  return tostring(a.resume()) .. "|" .. tostring(a.status())
end
go()
"#,
    );
    assert!(
        out.starts_with("b gave nil and failed with: "),
        "got: {out}"
    );
    assert!(out.contains("waiting on another"), "got: {out}");
    assert!(out.ends_with("|suspended"), "got: {out}");
}

#[test]
fn yielding_from_inside_a_callback_fails_the_coroutine() {
    let out = text(
        r#"
fn go() -> string
  local gen: any = Coroutine.create(fn(co: any) -> nil
    local t: table<integer> = {2, 1}
    Table.sort(t, fn(a: integer, b: integer) -> boolean
      co.yield(a)
      return a < b
    end)
  end)
  local r: any? = gen.resume()
  return tostring(r) .. "|" .. tostring(gen.status()) .. "|" .. tostring(gen.error())
end
go()
"#,
    );
    assert!(out.starts_with("nil|dead|"), "got: {out}");
    assert!(out.contains("inside a callback"), "got: {out}");
}

#[test]
fn yielding_from_inside_a_tostring_overload_fails_the_coroutine() {
    let out = text(
        r#"
class Loud implements OpToString
  local co: any
  fn init(co: any)
    self.co = co
  end
  fn toString() -> string
    self.co.yield("from toString")
    return "loud"
  end
end
fn go() -> string
  local gen: any = Coroutine.create(fn(co: any) -> nil
    local l: Loud = Loud(co)
    local s: string = "" .. l
  end)
  gen.resume()
  return tostring(gen.status()) .. "|" .. tostring(gen.error())
end
go()
"#,
    );
    assert!(out.starts_with("dead|"), "got: {out}");
    assert!(out.contains("inside a callback"), "got: {out}");
}

#[test]
fn a_yielder_used_outside_its_coroutine_is_an_error() {
    let e = fails(
        r#"
fn go() -> string
  local holder: table<any> = {}
  local gen: any = Coroutine.create(fn(co: any) -> nil
    holder[1] = co
    co.yield(nil)
  end)
  gen.resume()
  local y: any = holder[1]
  y.yield(1)
  return "unreachable"
end
go()
"#,
    );
    assert!(e.contains("not the one running"), "got: {e}");
}

#[test]
fn one_coroutine_cannot_yield_another() {
    // `b` is handed `a`'s yielder and uses it while `a` is waiting on `b`.
    let out = text(
        r#"
fn go() -> string
  local gen: any = Coroutine.create(fn(co: any) -> nil
    local bBody: any = fn(bco: any, other: any) -> nil
      other.yield("wrong one")
    end
    local b: any = Coroutine.create(bBody)
    b.resume(co)
    co.yield("b: " .. tostring(b.error()))
  end)
  return tostring(gen.resume())
end
go()
"#,
    );
    assert!(out.starts_with("b: "), "got: {out}");
    assert!(out.contains("not the one running"), "got: {out}");
}

#[test]
fn a_yielder_whose_coroutine_is_gone_says_so() {
    let e = fails(
        r#"
fn go() -> string
  local holder: table<any> = {}
  local gen: any? = Coroutine.create(fn(co: any) -> nil
    holder[1] = co
    co.yield(nil)
  end)
  gen!.resume()
  gen = nil
  local y: any = holder[1]
  y.yield(1)
  return "unreachable"
end
go()
"#,
    );
    assert!(e.contains("no longer exists"), "got: {e}");
}

#[test]
fn create_rejects_what_is_not_a_function_at_run_time_too() {
    // Written directly, `Coroutine.create(42)` is a type error. Through an
    // `any` the checker cannot see it, and the runtime has to say the same.
    let e = fails("local f: any = 42\nlocal gen: any = Coroutine.create(f)\n1");
    assert!(e.contains("expects a function"), "got: {e}");
}

#[test]
fn yield_is_not_a_method_of_the_coroutine() {
    let e = fails(
        r#"
local gen: any = Coroutine.create(fn(co: any) -> nil
end)
gen.yield(1)
1
"#,
    );
    assert!(
        e.contains("`yield` is not a method of the coroutine"),
        "got: {e}"
    );
}

#[test]
fn unknown_methods_and_wrong_arities_are_errors() {
    let e = fails(
        r#"
local gen: any = Coroutine.create(fn(co: any) -> nil
end)
gen.frobnicate()
1
"#,
    );
    assert!(
        e.contains("`Coroutine` has no method `frobnicate`"),
        "got: {e}"
    );
    let e = fails(
        r#"
local gen: any = Coroutine.create(fn(co: any) -> nil
end)
gen.status(1)
1
"#,
    );
    assert!(e.contains("takes no arguments"), "got: {e}");
}

#[test]
fn calling_a_yielder_method_that_does_not_exist_fails_the_coroutine() {
    let out = text(
        r#"
fn go() -> string
  -- Held as `any`, so `co` is too: typed, this is a compile error instead.
  local body: any = fn(co: any) -> nil
    co.resume()
  end
  local gen: any = Coroutine.create(body)
  gen.resume()
  return tostring(gen.error())
end
go()
"#,
    );
    assert!(
        out.contains("`Yielder` has no method `resume`"),
        "got: {out}"
    );
}

// ── Identity ─────────────────────────────────────────────────────────────

#[test]
fn coroutines_and_yielders_have_their_own_type_and_identity() {
    assert_eq!(
        text(
            r#"
fn go() -> string
  local seen: table<any> = {}
  local gen: any = Coroutine.create(fn(co: any) -> nil
    seen[1] = co
    co.yield(nil)
    co.yield(co == seen[1])
  end)
  local other: any = Coroutine.create(fn(co: any) -> nil
  end)
  gen.resume()
  local sameYielder: string = tostring(gen.resume())
  local y: any = seen[1]
  return type(gen) .. "," .. type(y) .. "," .. tostring(gen) .. "," .. tostring(y) .. ","
    .. tostring(gen == gen) .. "," .. tostring(gen == other) .. "," .. sameYielder
end
go()
"#
        ),
        "coroutine,yielder,<coroutine>,<yielder>,true,false,true"
    );
}

#[test]
fn equality_agrees_however_it_is_compiled() {
    // `==` in a condition and `==` as a value compile to different opcodes;
    // they must agree on coroutines as they do on everything else.
    assert_eq!(
        text(
            r#"
fn go() -> string
  local a: any = Coroutine.create(fn(co: any) -> nil
  end)
  local b: any = a
  local asValue: boolean = a == b
  local inCondition: string = "no"
  if a == b then
    inCondition = "yes"
  end
  return tostring(asValue) .. inCondition
end
go()
"#
        ),
        "trueyes"
    );
}

// ── Memory ───────────────────────────────────────────────────────────────

/// Pull the coroutine out of a program's result.
fn coroutine_of(v: Value) -> Rc<saule_runtime::value::CoroutineRef> {
    match v {
        Value::Coroutine(c) => c,
        other => panic!("expected a coroutine, got {other:?}"),
    }
}

#[test]
fn a_suspended_coroutine_nothing_refers_to_is_freed() {
    // Its body holds its yielder in a register for as long as it is
    // suspended; were that the coroutine itself, this would never be freed.
    let co = coroutine_of(eval(
        r#"
local gen: any = Coroutine.create(fn(co: any) -> nil
  local keep: table<integer> = {1, 2, 3}
  co.yield(1)
  co.yield(2)
end)
gen.resume()
gen
"#,
    ));
    let weak = Rc::downgrade(&co);
    drop(co);
    assert!(weak.upgrade().is_none(), "a suspended coroutine leaked");
}

#[test]
fn fresh_and_dead_coroutines_are_freed_too() {
    for src in [
        "local gen: any = Coroutine.create(fn(co: any) -> nil\nend)\ngen",
        "local gen: any = Coroutine.create(fn(co: any) -> nil\nend)\ngen.resume()\ngen",
        "local gen: any = Coroutine.create(fn(co: any) -> nil\n  throw \"x\"\nend)\ngen.resume()\ngen",
    ] {
        let co = coroutine_of(eval(src));
        let weak = Rc::downgrade(&co);
        drop(co);
        assert!(weak.upgrade().is_none(), "leaked: {src}");
    }
}

#[test]
fn a_suspended_coroutine_does_not_keep_the_program_alive() {
    // It must not hold the engine's shared state strongly, or a coroutine
    // stored in a module slot would leak the whole program. Once the run is
    // over, resuming it finds the engine gone — and says so.
    let v = eval(
        r#"
local gen: any = Coroutine.create(fn(co: any) -> nil
  co.yield(1)
  co.yield(2)
end)
gen.resume()
gen
"#,
    );
    let err = saule_runtime::call::call_method(&v, "resume", &[], 0..0).unwrap_err();
    assert!(
        format!("{err}").contains("whose program has finished running"),
        "got: {err}"
    );
}

// ── Scale ────────────────────────────────────────────────────────────────

#[test]
fn a_generator_yields_a_hundred_thousand_values() {
    assert_eq!(
        text(
            r#"
fn go() -> string
  local gen: any = Coroutine.create(fn(co: any) -> nil
    for i = 1, 100000 do
      co.yield(i)
    end
  end)
  local sum: integer = 0
  local v: any? = gen.resume()
  while v != nil do
    sum = sum + (v as integer)!
    v = gen.resume()
  end
  return "" .. sum
end
go()
"#
        ),
        "5000050000"
    );
}

#[test]
fn a_thousand_coroutines_are_created_and_finished() {
    assert_eq!(
        text(
            r#"
fn go() -> string
  local total: integer = 0
  for i = 1, 1000 do
    local body: any = fn(co: any, n: integer) -> integer
      co.yield(n)
      return n * 2
    end
    local gen: any = Coroutine.create(body)
    gen.resume(i)
    total = total + (gen.resume() as integer)!
  end
  return "" .. total
end
go()
"#
        ),
        "1001000"
    );
}

#[test]
fn coroutines_nested_fifty_deep_unwind_correctly() {
    on_a_real_stack(|| {
        assert_eq!(
            text(
                r#"
fn dive(depth: integer) -> string
  if depth == 50 then
    return "bottom"
  end
  local body: any = fn(co: any) -> string
    co.yield(depth)
    return dive(depth + 1)
  end
  local gen: any = Coroutine.create(body)
  gen.resume()
  return tostring(gen.resume())
end
dive(0)
"#
            ),
            "bottom"
        );
    });
}

#[test]
fn unbounded_nesting_of_resumes_is_stopped_and_reported() {
    // Every resume nests a dispatch loop on the native stack. Runaway
    // nesting has to end in the stack guard's diagnostic — reported up the
    // chain of coroutines, each of which fails in turn — not in a crash.
    on_a_real_stack(|| {
        let out = text(
            r#"
fn dive(depth: integer) -> string
  local body: any = fn(co: any) -> string
    return dive(depth + 1)
  end
  local gen: any = Coroutine.create(body)
  local r: any? = gen.resume()
  if r == nil then
    return "stopped: " .. tostring(gen.error())
  end
  return tostring(r)
end
dive(0)
"#,
        );
        assert!(out.starts_with("stopped: "), "got: {out}");
        assert!(
            out.contains("stack") || out.contains("nested"),
            "expected the depth guard's message, got: {out}"
        );
    });
}
