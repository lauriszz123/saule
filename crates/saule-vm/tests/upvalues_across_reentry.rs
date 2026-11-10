//! Captured locals seen from a callback that runs on another register file.
//!
//! A native that invokes a Saule function — `Table.sort`'s comparator, an
//! operator overload, a `toString` — runs it on a *fresh* `Vm`, with a
//! register file of its own. An upvalue that is still **open** points at a
//! register by index, and that index only means something in the register
//! file the capturing frame lives in. Before this was fixed, a comparator
//! that wrote a captured local wrote *its own* register at that index
//! instead: the caller never saw the write, and the callback's own
//! temporaries were silently clobbered.
//!
//! The rule the VM now keeps is that every open upvalue points into the
//! register file that is running. Before control leaves the dispatch loop
//! for runtime code that may call back in, the VM parks its open upvalues in
//! their cells; when control comes back, it writes them back and reopens
//! them. Each test here drives one of the exits that can call back in.
//!
//! Every captured local below lives inside a function on purpose: a
//! top-level local is a module slot, not a register, and would pass whether
//! or not upvalues worked.

use saule_lexer::Lexer;
use saule_parser::parse;
use saule_runtime::Value;

fn eval(src: &str) -> Value {
    let toks = Lexer::new(src).tokenize().expect("lex");
    let mut module = parse(toks).expect("parse");
    match saule_vm::check_and_run(&mut module, "test.sau", src) {
        Ok(v) => v,
        Err(e) => panic!("pipeline failed: {e:?}\n--- source ---\n{src}"),
    }
}

fn text(src: &str) -> String {
    match eval(src) {
        Value::Str(s) => (*s).clone(),
        other => panic!("expected a string, got {other:?}"),
    }
}

#[test]
fn a_comparator_writes_a_captured_local_the_caller_then_reads() {
    // The reported shape. The count was 0 before the fix.
    assert_eq!(
        text(
            r#"
fn go() -> string
  local count: integer = 0
  local t: table<integer> = {3, 1, 2}
  Table.sort(t, fn(a: integer, b: integer) -> boolean
    count = count + 1
    return a < b
  end)
  return tostring(count > 0) .. ":" .. t[1] .. t[2] .. t[3]
end
go()
"#
        ),
        "true:123"
    );
}

#[test]
fn a_comparator_reads_a_write_made_after_it_was_built() {
    // Descending only if the comparator sees `flip` as it is *now*.
    assert_eq!(
        text(
            r#"
fn go() -> string
  local flip: boolean = false
  local cmp = fn(a: integer, b: integer) -> boolean
    if flip then
      return a > b
    end
    return a < b
  end
  flip = true
  local t: table<integer> = {2, 8, 4}
  Table.sort(t, cmp)
  return t[1] .. "," .. t[2] .. "," .. t[3]
end
go()
"#
        ),
        "8,4,2"
    );
}

#[test]
fn the_binding_stays_live_in_both_directions_after_the_callback() {
    // After the native returns, the upvalue has to be open again: a write
    // by the frame must reach the closure, and a write by the closure must
    // reach the frame — the ordinary live-binding rule, resumed.
    assert_eq!(
        text(
            r#"
fn go() -> string
  local n: integer = 0
  local bump = fn(a: integer, b: integer) -> boolean
    n = n + 1
    return a < b
  end
  local t: table<integer> = {2, 1}
  Table.sort(t, bump)
  local afterSort: integer = n
  n = 100
  bump(1, 2)
  return afterSort .. "/" .. n
end
go()
"#
        ),
        "1/101"
    );
}

#[test]
fn a_tostring_overload_reaches_a_captured_local() {
    // `..` renders through `display_into`, which runs `toString` on a
    // fresh register file. The field holds a closure over `go`'s locals.
    assert_eq!(
        text(
            r#"
class Label implements OpToString
  local render: fn() -> string
  fn init(r: fn() -> string)
    self.render = r
  end
  fn toString() -> string
    return self.render()
  end
end
fn go() -> string
  local word: string = "before"
  local hits: integer = 0
  local l: Label = Label(fn() -> string
    hits = hits + 1
    return word
  end)
  word = "after"
  local s: string = "[" .. l .. "]"
  local t: string = tostring(l)
  return s .. t .. hits
end
go()
"#
        ),
        "[after]after2"
    );
}

#[test]
fn an_operator_overload_reaches_a_captured_local() {
    // `ARITHX`: the receiver is not proved, so the overload is found and
    // run by the runtime, re-entrantly.
    assert_eq!(
        text(
            r#"
class Acc implements OpAdd<Acc, Acc>
  local n: integer
  local onAdd: fn() -> nil
  fn init(n: integer, onAdd: fn() -> nil)
    self.n = n
    self.onAdd = onAdd
  end
  fn add(other: Acc) -> Acc
    self.onAdd()
    return Acc(self.n + other.n, self.onAdd)
  end
end
fn go() -> string
  local adds: integer = 0
  local f = fn() -> nil
    adds = adds + 1
  end
  local a: any = Acc(1, f)
  local b: any = Acc(2, f)
  local c: any = a + b
  return adds .. ""
end
go()
"#
        ),
        "1"
    );
}

#[test]
fn a_dynamic_method_call_reaches_a_captured_local() {
    // `CALLMX`: a method call on an `any` receiver, dispatched by name.
    assert_eq!(
        text(
            r#"
class Box
  local cb: fn() -> nil
  fn init(cb: fn() -> nil)
    self.cb = cb
  end
  fn poke()
    self.cb()
  end
end
fn go() -> string
  local pokes: integer = 0
  local b: any = Box(fn() -> nil
    pokes = pokes + 1
  end)
  b.poke()
  b.poke()
  return pokes .. ""
end
go()
"#
        ),
        "2"
    );
}

#[test]
fn a_closure_built_inside_a_callback_keeps_both_bindings() {
    // The comparator builds a closure over *its own* local and over the
    // caller's, and that closure escapes. Called later, from the caller's
    // register file, it must still see both.
    assert_eq!(
        text(
            r#"
fn go() -> string
  local outer: integer = 1
  local escaped: fn() -> integer = fn() -> integer
    return 0
  end
  local t: table<integer> = {2, 1}
  Table.sort(t, fn(a: integer, b: integer) -> boolean
    local mine: integer = 40
    escaped = fn() -> integer
      return mine + outer
    end
    return a < b
  end)
  outer = 2
  return escaped() .. ""
end
go()
"#
        ),
        "42"
    );
}

#[test]
fn a_throw_out_of_a_callback_with_open_upvalues_is_reported_cleanly() {
    // The callback unwinds instead of returning, with the caller's upvalues
    // parked and one of its own captured into an escaping closure. Nothing
    // in the language can catch an error that escapes a native today, so
    // what this pins is that the path is an ordinary diagnostic — not an
    // internal fault from a register file left half-parked.
    //
    // Surviving such an error (and reading the escaped closure afterwards)
    // becomes expressible with coroutines, whose `resume` reports a failure
    // instead of propagating it; those tests live with the coroutine suite.
    let src = r#"
fn go() -> string
  local n: integer = 0
  local escaped: fn() -> integer = fn() -> integer
    return n
  end
  local t: table<integer> = {2, 1}
  Table.sort(t, fn(a: integer, b: integer) -> boolean
    local mine: integer = 42
    escaped = fn() -> integer
      return mine + n
    end
    n = 5
    throw "stop"
  end)
  return "unreachable"
end
go()
"#;
    let toks = Lexer::new(src).tokenize().expect("lex");
    let mut module = parse(toks).expect("parse");
    let err = saule_vm::check_and_run(&mut module, "test.sau", src).unwrap_err();
    let msg = format!("{err:?}");
    assert!(
        msg.contains("comparator failed") && msg.contains("stop"),
        "expected the comparator's throw, got: {msg}"
    );
}

#[test]
fn nested_callbacks_each_see_their_own_callers_locals() {
    // Two register files deep: a comparator that sorts with a comparator,
    // each writing a local of the frame that built it.
    assert_eq!(
        text(
            r#"
fn go() -> string
  local outerHits: integer = 0
  local innerHits: integer = 0
  local t: table<integer> = {2, 1}
  Table.sort(t, fn(a: integer, b: integer) -> boolean
    outerHits = outerHits + 1
    local u: table<integer> = {4, 3}
    Table.sort(u, fn(x: integer, y: integer) -> boolean
      innerHits = innerHits + 1
      return x < y
    end)
    return a < b
  end)
  return tostring(outerHits > 0 and innerHits >= outerHits)
end
go()
"#
        ),
        "true"
    );
}
