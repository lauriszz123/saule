# Saule VM — Performance Task List

> Closing the remaining gap to Lua. `VM_TASKS.md` is the feature checklist;
> this one is the *speed* checklist, and every item on it is justified by a
> profile rather than by intuition.
>
> **Ground rule, inherited from `VM_TASKS.md` and absolute:** all of these
> pass at every commit. (The tree-walking interpreter this file used to keep
> green has been removed; its answers live on as the recorded outputs the
> first two compare against — see `VM_TASKS.md`.)
>
> ```
> ./run_tests.sh                       # every fixture prints what it recorded
> bash run_examples.sh                 # multi-module projects, likewise
> python3 benchmarks/bench.py check    # still agrees with Lua
> ```

## Where we are

Measured against Lua 5.5, min-of-9 interleaved, geomean excluding `startup`:

| | session start | now |
| --- | --- | --- |
| **geomean** | **1.73x** | **1.30x** |
| `map` | 1.20x | **0.46x** |
| `strings` | 1.80x | 1.02x |
| `mandel` | 1.79x | 1.10x |
| `wordfreq` | 1.56x | 1.18x |
| `json` | 1.50x | 1.22x |
| `closure` | 3.00x | 2.00x |
| `fib` | 3.09x | 2.43x |

Landed so far: table growth 2x→3x (`TableObject::grow_hint`), integer
rendering without `core::fmt` (`saule_runtime::itoa`), single-buffer
concatenation (`display_into`/`display_hint`), four fat dispatch arms moved
out of line, and the register write skipping `Value`'s drop glue
(`Vm::set_reg`).

### The drop glue on every register write ✓ DONE

Found by disassembling `execute_loop` rather than by sampling it, which is
why it survived this long: `sample` attributes the call to the arm that
makes it, so the profile said "arithmetic" and looked reasonable.

`*self.reg_mut(i) = Value::Int(x)` drops what the register held. The
compiler cannot prove that what it held was an `Int`, so every `ADDII`,
`SUBII`, `MULII`, `FORLOOP_I` and load ended in an unconditional
`bl drop_in_place<Value>` — a call, on the hottest path there is, to run
nothing. [`Value::is_scalar`] settles it inline instead, and
[`Vm::set_reg`] keeps the real drop for the case that needs one. It is the
mirror of the split `clone`/`clone_heap` already had on the copy side; only
the drop side had been left out.

Measured min-of-9 interleaved, against the same binary without it:
`loop_arith` **0.75x**, `mandel` **0.90x**, `bintree` **0.91x**, the rest at
parity. No fixture, example or benchmark output changed.

### A loop over an array no longer copies it ✓ DONE

Found in a profile of a real program (the window manager of Saule Linux,
idle), not of a benchmark: none of the thirty-one loops over a list of
objects the way a view tree is walked every frame.

`for v in t` has to see `t` as it was when the loop began (LANGUAGE §11.2),
and `ITERPREP` got that by copying every key and value into a new table
before the first step: two allocations, and a reference count taken and
given back per element, for every loop. A table that is all array is now
gone over where it is. The array part (`saule_runtime::value::ArrayPart`)
hands the loop a *lease* and copies itself into it only if it is written to
while a loop still holds it; an array nothing writes to keeps its lease, and
looping over it again allocates nothing. Tables with entries outside the
array are copied and sorted as before.

The step itself was the other half: handing the element on as an
`Option<Value>` sent it through the stack in pieces, and reading it back
whole was a third of the step.

Measured on a list of sixteen objects, summing a field: **28 → 15 ns** an
element (Lua 5.4's `ipairs`: 18). `bench.py` old against new: level on every
benchmark, so the test every `&mut` access to an array now makes costs
nothing that shows.

### A call into a native package ✓ DONE (in part)

Three allocations a call are now one: the arguments are marshalled on the
stack (up to eight), and one result is put straight into the vector that
carries it. A result is matched against its declared type only where that
type mentions one of the package's enums, which is asked once per native.
`Timer.getTime()` from Saule: **86 → 67 ns**. What is left is four
thread-local accesses a call (`native_host::enter`/`exit` and the package
stack) and, on the package's side, a `String` allocated for every string
argument.

### `Math` calls: the name written out for every call ✓ DONE

`mathlib` was the furthest behind Lua of all thirty-one, at 2.2x, and a
third of it was this: `Math`'s own `expect_arity` wrote `Math.{name}` into
a new `String` — for the message of a call with the wrong number of
arguments — before looking at how many there were. Every `Math` call made
a `String` and freed it. The count is looked at first now.

The rest of the gap that could be had without new opcodes: a native that
only works something out from its arguments has a shorter way in
(`NativeFn::quick`), which is the native itself with its error let go of,
so the two cannot disagree. The VM takes it first and skips what a call
sets up in case a native runs Saule code; a call the native refuses goes
the ordinary way, for the error.

`mathlib`: **0.676 → 0.376 s**, 2.2x → 1.2x of Lua. What is left is the
call itself, thirteen to an iteration: `Math.sqrt` of a `float` compiled
to an opcode of its own would be the next step.

`treewalk` is new with this: a tree of objects walked child list by child
list, which nothing in the suite did. 1.33x of Lua before the loops stopped
copying, 0.9x after.

### Coroutines: a switch that takes nothing apart ✓ DONE (in part)

`coroutine`, `coro_pipe` and `coro_tree` were 2.1x, 2.0x and 1.8x of Lua.
Three things, in the order they were found:

* A suspended coroutine's register file was a `Vm` **taken apart into its
  fields** and boxed, and built again around them at the next resume: the
  `Vm` copied four times and an allocation made and freed per round trip,
  a fifth of what one cost. It is kept whole now. What that was for — a
  suspended coroutine must not hold the program's `VmShared`, or one kept
  in a module slot leaks the program — is done by swapping in the shared
  state of no program (`VmShared::nobody`) while it is suspended.
* `yield` found out it was a yield **on its way through the dynamic call**:
  the method's name cloned, the yielder's coroutine looked up through its
  weak reference (taken and given back), and then out again as an error
  through `native_throw`, which went to a thread-local to ask whether a
  native had thrown. The coroutine's own register file asks first now
  (`Vm::yield_at`), the yielder knows its coroutine's address, and a
  `Yield` is not asked about.
* The values yielded were collected in a vector that was given away each
  time and grown again from nothing.

The test stays in `call_member_by_name` and out of the `CALLMX` arm: in
the arm it was worth 3% more on `coroutine` and cost `oop`, which makes no
such call, 8% — the dispatch loop's usual answer to anything put in it.

`coroutine` **0.49 → 0.36 s**, `coro_pipe` 0.45 → 0.33, `coro_tree`
0.38 → 0.32: about 1.45x of Lua each. What is left is the loop's side of it: a
coroutine in a `for … in` is stepped through a boxed native closure that
returns a `Vec`, with the driver copied into the call window at every
step. A step opcode of its own would take most of that.

### `generic`: no cost of its own, and two things on the way ✓ DONE (in part)

`generic` was the furthest behind Lua once `mathlib` and the coroutines
had moved, at 1.82x. Its bytecode has nothing generic in it — a `Stack<T>`
compiles to the instructions a `Stack` of integers would — so what it
measures is six calls an iteration: four method calls, a static one and a
closure passed as an argument. Timed apart, against Lua: an empty method
call 24 ns to 16, a static call 19 to 8, `self.items[i] = v` in a method
39 to 20. The gap is the call, and reading a table out of a field to index
it.

Two things found on the way, neither of them `generic`'s alone:

* **A method-cache hit came back through memory.** `vtable_lookup_cached`
  returns `Result<(usize, u32), RuntimeError>`, written by the callee and
  read back by the `CALLM` arm on every method call, to carry an error a
  hit never has. `Vm::callee_packed` answers in one register and leaves
  the error of the rare failure in `Vm::fault`. `oop` 0.053 → 0.048.
  The first shape tried — the hit tested in the arm, the miss a second
  call — was the same on `oop` and cost `loop_arith` and `mandel` 10%,
  which make no method call: the arm had grown.
* **`GETF` ran a destructor for a number.** It wrote its register with a
  plain assignment, so every field read called `Value`'s drop glue for
  what the register held, which is nearly always a number (`set_reg`).
  `interp` 0.55 → 0.49, `entity` and `vecops` 5%.

`generic` itself: 0.200 → 0.195. What would move it is not here: an
instruction that indexes a table held in a field without copying the
table into a register first (`self.items[self.top]` is a reference count
taken and given back, through two calls, around every access), and a
cheaper call. The first is a new opcode, which is the chunk ABI's to
decide.

Also tried and put back: building a static call's frame from the target's
fields as it is written, so that the proto and chunk are not stored to the
stack apart and read back as one sixteen-byte value. The profile put a
quarter of `enter_static` on that read; the clock put 3% on a loop of
nothing but static calls and nothing on any benchmark.

### `self.items[i]` without copying the table out ✓ DONE

The instruction the `generic` entry above said was missing: `GETFIDX`,
`GETFIDXU` and `SETFIDX` index a table that `self` holds in a field where
it lies, read (with and without `!`) and written. `GETF` then `GETIDX`
copied the table into a register between the two — a reference count
taken, and given back when the register was next written, each through a
call — around every element a class read out of its own list.

The receiver is register 0, which is what leaves room in three operands
for the field's slot; emitted in a method, for a field declared as a
table, with an index (and a value) that can run no code. Appended after
`ISTYPE`. See the opcodes' own notes in `op.rs`.

**One arm in the dispatch loop for the three.** An arm each — each only a
call to an out-of-line function — was `generic` −5% and `interp` −4%, and
`fib` +6%, `mandel` +8%, `loop_arith` +2.5%, none of which run them. One
arm, `GETFIDX | GETFIDXU | SETFIDX`, calling one function that tells them
apart, keeps the gain and leaves the rest where they were.

`generic` 0.195 → 0.184 (1.75x of Lua at the start, 1.60x now), `interp`
0.493 → 0.474.

### `nullsafe`: a path's value went round through memory three times ✓ DONE

`nullsafe` is `GETPATH` after `GETPATH` — `row.mid?.inner?.v`,
`row.mid!.inner != nil` — and the walk itself was already what it should
be: every link read in place, no object in the middle copied out. What
cost a fifth of the benchmark was the end of it. `follow_path` answered
`Result<Value, RuntimeError>`, and a path that ends at an object or a
table (every nil test on one, every `?.tags`) had its clone written to
the stack, copied into the `Result`, read back out and written to the
stack again before it reached its register.

`follow_path` answers where the value is now, and `Vm::path_into` clones
it once, straight to the register, counting an object's or a table's
reference in line. It is out of line and answers a `bool`, with the error
in `Vm::fault`: the same change made in the arm was the same 13% on
`nullsafe` and cost `loop_arith`, `fib` and `mandel` 8 to 10%.

`nullsafe` **0.240 → 0.208 s**, 1.75x → 1.5x of Lua. What is left is the
copies themselves: `if row.mid != nil` reads the object into a register,
counted, to look at whether it is nil. A test that looks without reading
would be an opcode of its own.

### A nil test on a field without reading the field ✓ DONE

What the `nullsafe` entry above left: `if row.mid != nil` read the object
into a register, counted, to see whether it was nil. `ISNILP` walks the
path (`GETPATH`'s, one link included) and writes only the answer; a `TEST`
branches on it. Emitted by `nil_compare_jump` for any chain of field reads
compared with nil in a condition.

`nullsafe` 0.208 → 0.170 s (1.74x of Lua at the start, 1.24x now) and
`bintree` 0.415 → 0.360 (every `if node.left != nil`), with `mandel`,
`loop_arith`, `fib`, `generic`, `interp` and `json` where they were.
**`oop` is 6% slower with it** (0.047 → 0.050, still under the 0.052 it
began at), and so are `coroutine` (0.358 → 0.378) and `coro_pipe`
(0.328 → 0.345), which keep most of what they had gained. None of them
runs such an instruction: the arm it shares with `GETFIDX`
is unchanged, so this is the jump table being one entry longer or the
out-of-line function being larger. Moving the arm to the end of the
`match` did not bring it back.

It was a jump first (`JNILP`, skipping the next instruction itself, one
instruction fewer). Four shapes of arm — its own, with the test in line;
shared, answering 0, 1 or 2 through a `match`; shared, with `pc += skip`;
its own again beside `JNIL` — were each a little faster on the two that
use it and each `mandel` +12%, with `oop` +9% or `loop_arith` +7% beside
it. What they had in common was an arm that changes `pc` after a call.

### `Table.sort` by a plain comparison ✓ DONE

`sort` was 1.75x of Lua, and nine tenths of it was not sorting:
`(a, b) => a < b` is two instructions (`LTI`, `RET1`), and each of the three
and a half million times a sort of 200 000 numbers asked it, a frame was
pushed and popped around them (`push_frame_resolved` 29%, `pop_frame` 24%,
`call_first` 16% of the run). That path had already had its allocations,
its pool round trip and its downcast taken out; what was left was the call.

So it is not made. A call session says what its function is when it is one
typed comparison of its two parameters (`CallSession::compares`, told from
the proto by `frame::comparison_of`: `LTI`, `LEI` or `LTF` over registers 0
and 1, either way round, and a `RET1` of the answer), and `Table.sort`
sorts such values itself (`sort_plainly`): integers by any of the four
comparisons, floats by the two strict ones and only with no NaN among
them, where the order is the one the merge would give. Anything else, a
table with another kind of value in it included, is sorted by asking, as
before.

`sort` 0.150 → 0.018 s (0.21x of Lua), `tablelib` 0.205 → 0.145.
`tests/run/stdlib/table_sort_by_a_plain_comparison.sau` sorts the same
values both ways and compares.

A comparator that reads a field (`(a, b) => a.age < b.age`) is still
called; so is one over strings, which compiles to `ARITHX`.

### `match`: a failed arm goes to the next arm for its variant ✓ DONE

Under a `SWITCH` the first arm for a tag was already entered past its tag
test. An arm whose guard failed still went to the arm below, which tested
the tag again, and through every arm between it and the next one for the
same variant. Now no arm under a switch tests the tag: a failed guard or
payload jumps to the next arm for that variant, a wildcard, or the end
(`match_chain`'s `waiting`). A wildcard with a guard keeps the chain,
since after it fails the tag is anybody's.

And a `JMP` to a `RET0`, a `RET1` or another `JMP` does what that one does
(`peephole::thread_jumps`): every arm of a `return match …` ended in a jump
to the one return after the last arm.

`Main.handle` in `benchmarks/sau/match.sau` went from 37 instructions to
24, and a call of it runs 4 to 7 of them where it ran 5 to 11. **`match`
itself moved 4%** (0.181 → 0.173, 1.63x of Lua): the arms were a small part
of it. What is left is the call (`CALLSTAT`, `enter_static`, `RET1`) and
reading the payload, which is a table inside the variant, three objects a
value: with the same number of cache misses as Lua it runs at three
instructions a cycle where Lua runs at four.

### Registers through a pointer in the arms that only use registers ~ PARTLY

`loop_arith` ran 197 machine instructions an iteration, for four of
Saule's; Lua runs 135 for its four. Each arm found a register by loading
`self`, then the stack's pointer, then `base`, and adding them, per
operand. The arithmetic, move, constant, float and loop arms and `GETF`
now read and write through `regs`, a pointer to `R[0]` held across
instructions (`r!`, `w!`), made again after any other arm (which may have
grown the stack or changed `base`) and at the three places that change the
frame and `continue`. 148 instructions an iteration; every benchmark runs
1 to 24% fewer.

**In time it is worth less than that:** `loop_arith` and `mandel` about 12%
(0.043 → 0.037, 0.026 → 0.023), `fib` and `bintree` a few percent, the rest
level, `interp` and `strings` 2 to 4% slower. With `regs` in a register the
compiler moved `pc` out of one, so every instruction's fetch waits on a
store. Four other shapes were measured (`perf stat`, instructions and
cycles, every benchmark):

- with the compare-and-jump arms through `regs` too: `loop_arith` −25% and
  `mandel` −19% in cycles, and `fib` +15% (twice the mispredicted
  branches), `coroutine` +7%;
- `regs` made again for every instruction, held nowhere: 1 to 5% more
  instructions on most of the suite;
- `here` kept in the `Vm` and not in a local: `pc` back in a register,
  `loop_arith` −17%, and `strings` +8%, `interp` +6%;
- every function, or every jump target, aligned to 64 or 32 bytes
  (`-align-all-functions`, `-align-all-nofallthru-blocks`): no steadier.

Instruction counts are the same from run to run and cycles are within 2%,
so the instrument is sound; what it shows is that at an unchanged
instruction count a benchmark moves 5 to 15% with how the loop is laid
out. The next step is the one the note on dispatch below ends on: an
instruction pointer and a register pointer that the loop cannot spill,
which means a loop that is not one function.

### Small methods and choosing functions compiled where they are called ✓ DONE

`generic` has nothing generic in its bytecode (see the entry above): it is
six calls an iteration. What the compiler knows before the program runs is
most of each of them, and it now uses it.

**A small method, behind a test of the receiver's class**
(`compile::inline`). `st.push(x)` is `push`'s own instructions laid down
in the caller's registers from the receiver on (a method's frame starts
at its receiver, so nothing is shuffled), after one new instruction,
`ISREADY`, asks what cannot be known earlier: that the object is of
exactly the class the type checker proved, a subclass not being it, and
that the table the body indexes is there. Any other receiver takes the
`CALLM` kept beside the body. Only a body that cannot fail once the test
has passed is taken, so that no stack trace is a frame short: moves and
constants, typed arithmetic that cannot divide by zero, comparisons and
jumps, fields of `self`, and an element of a table `self` holds in a
field. At most twelve instructions, from this module, compiled before the
call that uses it.

**A constructor that only stores** needs no test: the object was made
by the `NEW` on the line above. `Point(x, y)` is a `NEW` and two `SETF`.

**A static function that only chooses** (`inline_static_call`).
`Main.pickMax(a, b, better)`, where `pickMax` is `return` after at most
two `if c then return e end`, every expression a parameter, a call of a
parameter, or arithmetic that cannot fail, and `better` is a lambda this
function holds in a local for its whole life: the body is compiled in
place, with the lambda's own body where it is called
(`inline_lambda_call`, which was there). A generic function over "how to
compare" becomes the comparison: `LTI`, `TEST`, two `MOVE`.

`generic` 0.190 → 0.139 s (1.68x of Lua → 1.25x), 34% fewer machine
instructions; `oop` 0.048 → 0.033 (1.31x → 0.92x); `vecops` 0.325 → 0.281;
`interp` 0.489 → 0.443. The rest run the same instructions as before.

What is left in `generic` is the test, three instructions a call
(`ISREADY`, `TEST`, `JMP`), and the table read out of the field into a
register for each index (`GETFIDX` names register 0 without saying so,
and cannot be moved to another frame's registers). A local that holds
what a constructor made and is never assigned again needs no test of its
class; the names for that are there (`assigned_names`), and it is not
done.

A call site that sees several classes pays the test and then calls: two
instructions more than before. `entity`, whose `score` is overridden in
both subclasses, is level.

### `match`: the fields in the variant, and four instructions fewer a turn ✓ DONE

`match` ran 0.183 s against Lua's 0.104 after the routing above. It
executed *fewer* machine instructions than Lua and took half again as
many cycles: eleven dispatches a turn of the loop, and three objects read
for each event.

**The fields are in the variant.** `Event.Click(x, y)` was a variant
holding a table holding an array: three allocations, and `GETPAY` went
through all three. The fields are now in the variant object itself, one
or two in place and more in a slice (`EnumVariantObject::carrying`).
`.value` still answers with a table of them, made the first time it is
asked for and kept; it may be written to, so from then on it is what the
variant carries and `GETPAY` reads it as before. Building the 200 000
events went from 128M cycles to 101M (Lua: 137M).

**A function that is one `match` over its argument** is compiled where
it is called, by the same road as `pickMax` (`inline_static_call`): arms
that are variants with their fields named, guards and bodies that only
choose. The parameter has to be an enum by its declared type; a nullable
one keeps the call.

**An operand beside such a call is read where it is.** `total = total +
Main.handle(e)` copied `total` aside before a call that is no longer
made. `operand_is_pure` now asks `runs_in_place`, which reads the same
plan the emission does; `call_to_want` refuses to compile a call that
was promised in place and came out a call, so a divergence between the
two is a compile error and never a wrong program.

**`SWITCHTAG`** is `GETTAG` and `SWITCH` as one instruction: the tag was
read into a register nothing else looked at.

**An arm that is one of its fields**, `case Key(c) then c`, reads the
field straight into the destination (`field_to`), where it was bound to
a register and moved.

**A test that does not skip takes the `JMP` after it** (`or_jump!` in
the dispatch): the false branch of every `if` was a second trip through
the dispatch for the jump. This one is not `match`'s alone: `fib` -19%
cycles, `nullsafe` -20%, `bytes` -14%, the coroutine three -8 to -12%,
4.8% off the suite's geometric mean, with the instruction counts nearly
unchanged. The peephole already turned `test; JMP +1` round, so what is
left to fuse is the jump that goes somewhere.

`match` 0.183 → 0.107 s (1.76x of Lua → 1.03x); 746M cycles → 462M, Lua
469M. The loop is `GETIDXU`, `SWITCHTAG`, the arm, `ADDI`, `FORLOOP_I`.

What is left: each arm but the last ends in a `JMP` to the end of the
`match`, and the two arms for one variant each read the field the guard
needed.

## The profiles these tasks are built on

Leaf-weighted, `sample`, release build. **Do not re-measure these to start a
task** — they are recorded here so the next person doesn't have to.

**`fib`** — the worst row, and it is all call machinery:

```
execute_loop              59.0%
push_frame_resolved       12.9%   ┐
enter_static              12.6%   │  37% call machinery
pop_frame                  9.7%   │
drop_in_place<Frame>       2.4%   ┘
drop_in_place<Value>       3.8%
```

**`map`** — the string/table half:

```
execute_loop               474      TableObject::set           385
TableObject::get_str       251      free                       242
malloc                     211      memcmp                     127
reserve_rehash             123
                                    → ~35% allocator, ~31% table
```

## The refcount question, answered early so nobody re-opens it

Lua does not refcount: a `TValue` copy is a 16-byte move with no
bookkeeping. Saule's is an increment now and a decrement plus a zero-test
later, on every move that touches a heap variant. It is tempting to call
that *the* structural gap and reach for a garbage collector.

**The profiles say no.** Refcount traffic is ~6% of `fib`
(`drop_in_place<Value>` 3.8%, `drop_in_place<Frame>` 2.4%) — and `fib`'s
loop holds nothing but integers, which are never refcounted at all. The two
rows furthest from Lua, `fib` and `closure`, would barely move.

Task 2 makes the same point from the other side: two separate attempts to
take `Rc`s off the call path both *lost* ~10%, because what replaced them
was worse than what they cost.

So refcounting is not the wall, and this file is deliberately ordered that
way — the memory-management item (Task 4) is filed as a **correctness** fix
for a demonstrated leak, and the speed items go after hashing (Task 1, done)
and dispatch (Task 5).

## Two constraints that bound every design here

1. **`Value` must stay 16 bytes.** Measured by padding a variant and
   re-running the suite: 32 bytes costs **~2% geomean**, 24 bytes costs
   **~4%** (24 is not a power of two, so `Vec<Value>` indexing needs a
   multiply instead of a shift). Any new string representation must fit in
   an 8-byte payload — which rules out inline short-string storage and
   `Rc<str>` alike.
2. **`execute_loop` is dominated by its own code size.** This has now paid
   out three separate times: the `profile` feature's second copy (2-3%,
   recorded in `Cargo.toml`), removing a `Vec<String>` from `CONCAT`
   (double-digit percentages on benchmarks that never concatenate), and
   outlining four allocating arms (-4.8% geomean, every row improved). Any
   change here must be measured against the **whole suite**, never against
   the benchmark it targets.

---

## Task 2 — Frames by index, not by `Rc`

**Targets** `fib` (2.31x), `closure` (1.90x), `entity` (1.72x), `sort`
(1.59x). **Attacks** the 37% of `fib` above.

A `Frame` holds three `Rc`s — `func`, `proto`, `chunk` — so a call is three
increments and a return is three decrements with three zero-tests. It is
48 bytes pushed and popped per call.

None of that is necessary. Replace the pointers with the indices they were
resolved from:

```rust
struct Frame {
    module: u16,      // -> shared.chunks[module]
    proto_idx: u16,   // -> chunk.protos[proto_idx]
    base: u32, ret_to: u32, pc: u32, top: u32,
    n_ret: u8, n_args: u8,
}
```

48 bytes → 20, and **zero refcount traffic**. Proto and chunk are recovered
by two indexed loads at frame *activation* — once per call and once per
return, not per instruction.

`func` is the only real obstacle: three upvalue opcodes read it. But a
statically-resolved call — all of `fib`, and most real code — has its handle
living forever in `shared.closure_cache[module][proto_idx]`, reachable from
the same two indices the frame now stores. Only a closure carrying upvalues
needs a reference of its own:

```rust
enum FrameFunc { Static, Closure(Rc<VmFunctionRef>) }
```

### ✗ ABANDONED — both designs measured, both are losses

**Do not attempt a third variant without reading this.**

| attempt | geomean | `fib` | `closure` |
| --- | --- | --- | --- |
| `Frame.proto`/`chunk` as `ManuallyDrop<Rc<_>>` | **+10.4%** | +17% | +30% |
| `Frame` by `(module, proto_idx)`, 48B → 32B | **+11.1%** | +25% | +43% |

Two structurally different designs, the same regression, the same shape —
worst on exactly the call-heavy benchmarks they were built to help. That is
not noise, and the second run makes the mechanism clear.

**The frame's `Rc`s are not the cost; re-deriving what they point at is.**
`execute_loop` re-derives proto and chunk once per frame *activation* — and
an activation happens on every call **and every return**, so `fib` does it
~60M times. With the `Rc`s in the frame that is two loads from a cache line
that was written moments ago. With indices it becomes a dependent chase:

```
self.shared -> VmShared -> chunks Vec ptr -> [module] -> Chunk -> protos Vec ptr -> [idx]
```

Six serialized loads, each waiting on the one before. Three refcount
increments are *independent*, pipeline freely, and hit L1 — they lose to
pointer-chase latency by a wide margin. Removing work is not the same as
removing time.

**What this says about the profile.** `fib`'s 37% in the call machinery is
real, but it is not refcount traffic. It is register-file setup
(`claim_registers`), the missing-parameter nil fill, the 48-byte frame push
and pop, `entry_for`, and the `max_frames` check — which reads through
`self.shared`, a pointer chase of its own on every call and a candidate for
caching in `Vm` directly. Anyone returning to this task should price those,
and leave the three `Rc`s alone.

---

## Task 1 — Interned strings with a cached hash ✓ DONE

**Measured: -4.3% geomean, `map` -42.0%, `wordfreq` -6.0%, `json` -3.3%, no
regressions.** `map` went from 0.83x Lua to **0.46x** — better than twice
Lua's speed on string-keyed table work.

Landed as `saule_interpreter::value::str::SauleStr`, replacing
`Value::Str(Rc<String>)` across ~120 call sites. `Deref<Target = String>` is
what kept that tractable: `**s`, `(**s).clone()` and `s.len()` all still mean
what they meant. The existing intern table in `stdlib/string.rs` now hands
out `SauleStr`, so interned strings share a hash as well as an allocation,
and `EnumVariantObject`'s two name fields became `SauleStr` so reading
`.name` off an enum value is a refcount bump rather than an allocation.

The hash is **lazy** — `Cell<u32>`, `0` meaning "not yet" — so
`benchmarks/sau/strings.sau`, which builds 200k strings and uses none as a
key, pays nothing. `TableKeyRef::Str` carries `(text, hash)` so both probe
paths agree bit for bit, and `TableKey::from_value` clones an `Rc` where it
used to deep-copy a `String` on **every map insert**.

### The original plan, for reference

**Targets** `map`, `strings`, `wordfreq`, `json`, and every record-shaped
table. **Attacks** the ~35% allocator and ~31% table time above.

`Value::Str(Rc<String>)` is **two allocations** — the `RcBox`, and `String`'s
byte buffer — and two pointer hops to reach a byte. Nothing caches the hash,
so `TableObject::get_str` re-walks the bytes on every lookup and
`reserve_rehash` re-walks them again on every growth.

Lua interns short strings and caches the hash in the `TString` header. That
is why `map` and `wordfreq` were close to begin with.

```rust
struct StrHeader { strong: Cell<u32>, hash: u32, len: u32, /* bytes inline */ }
Value::Str(NonNull<StrHeader>)   // thin pointer — Value stays 16 bytes
```

One allocation, one hop, hash is a load. Equality tries the pointer first
(interned literals answer there), then the hash, then the bytes.

What it should reach:

- `TableObject::set` + `get_str` — hashing becomes a `u32` load
- `reserve_rehash` — reads the stored hash instead of re-walking
- allocator traffic — halved
- `TableKey::Str(String)` — currently a **deep string copy on every map
  insert**; becomes a refcount bump

A standalone reproduction of `map`'s inner loop put the thin representation
at roughly **17 points beyond the itoa win**.

### Measured dead ends — do not re-explore

- **Inline short strings** (`[u8; 22]`): -0.0%, +1.1%, -3.3% across three
  runs — indistinguishable from zero — against the ~2% `Value`-widening
  cost it forces. Net loss.
- **`Rc<str>`**: +19% on the same workload. A fat pointer *and* it copies
  bytes on construction.
- **Caching the hash inside `TableKey`** rather than the string: worth ~2%
  on top of the 3x growth change, which does not pay for reshaping a `pub`
  enum that also carries the `Ord` iteration-order contract.

---

## Task 3 — Move the remaining per-execution work into the compiler ~ PARTLY DONE

- **Pre-hashed table keys — ✓ delivered by Task 1, for free.**
  `GETMAPK`/`SETMAPK` index `chunk.constants`, and a constant is now a
  `SauleStr`. Its hash is therefore computed once for the entire program run
  and read on every access after. No opcode or chunk-format change was
  needed; this is most of `wordfreq`'s -6.0% and `json`'s -3.3%.
- **`NEWVAR`'s two `String` allocations — ✗ not worth doing.** The field
  types are now `SauleStr`, but `new_variant` still builds them from
  `chunk.enums[..].name.to_string()`. Making that free means pre-building
  the names in the chunk, which touches all seven `EnumObject` construction
  sites — **for zero measurable benefit**, because `NEWVAR` only fires for
  *tuple* variants and every enum in the benchmark suite (including
  `interp.sau`, the enum-heavy one) uses unit variants, which come from
  cached singletons. Do it when a benchmark exercises it, not before.
- **`entry_for(n_args)` per call — ✗ not attempted.** It is on the call
  path, and Task 2 is two pieces of evidence that this area punishes
  changes. Price it against the whole suite before believing anything.
- **`CALLMX`'s constant clone — ✗ not attempted.** Now a refcount bump
  rather than a `String` copy anyway, and the opcode is rare.

### The original plan, for reference

The constant-pool principle: if the compiler knows it, the VM should not
compute it. Rides along with Task 1's interned pool.

- **Pre-hashed table keys.** `GETMAPK`/`SETMAPK` already take a *constant
  index*. Make that constant a pre-built `TableKey` carrying its hash, and
  literal-key access skips both the string clone and the hash entirely.
  `wordfreq`, `json` and record-shaped code are the beneficiaries;
  `map.sau` is not, because its keys are computed.
- **`NEWVAR` allocates a `String` per execution** —
  `chunk.enums[e].variants[tag].name.to_string()`, plus `e.name.clone()`.
  Both are compile-time constants and belong in the interned pool.
- **`entry_for(n_args)`** is resolved per call, but arity is static at each
  call site; it can be folded into the call instruction.
- **`CALLMX`** clones `chunk.constants[k]` for a name the compiler knows.

---

## Task 4 — A cycle collector ✓ DONE

**The leak is fixed and the cost is +1.6% geomean.** Landed as
`saule_interpreter::gc`.

```
                        before      after
n =   200,000 cycles     50 MB       7 MB
n = 1,000,000 cycles      —          7 MB
n = 4,000,000 cycles      —          7 MB
```

Flat across a 20x range: bounded, not merely smaller. Both engines
(`--vm` 7 MB, `--interp` 8 MB).

### What was built

A registry, not a `Gc<T>` — for the two reasons below, which are what make
textbook Bacon–Rajan the wrong shape here. `Rc` is untouched, so there is no
call-site churn, no rooting, no safepoints, and no ABI change.

* A container is registered when it is **stored into another container** —
  `TableObject::set` and `InstanceObject::set_field`, plus the VM's
  `SETFIELD`, are the three choke points. A table of integers never
  registers at all.
* Collection computes, per registered node, how many of its `Rc` references
  come from other registered nodes; anything with no external reference and
  unreachable from one is a cycle, and gets its contents cleared so ordinary
  refcounting reclaims it.
* Registering on *store* is sufficient because every node in a cycle is, by
  definition, stored inside another node of that cycle.

**The safety property is never freeing something reachable**, which reduces
to never over-counting internal references. The traversal walks only
`array`/`map`/`fields` — exactly the `Rc`s those objects hold — and counts
only *direct* `Value::Table`/`Value::Instance` children. Everything else
(`EnumVariant` payloads, `Function`, `VmFunction`) is **opaque**, so a
reference held through one counts as external and its cycle survives.
Under-collecting is safe; over-collecting would not be. `collects_only_what_it_can_prove`
pins that limit as deliberate rather than an oversight.

Seven unit tests, including three that assert the collector *declines* to
free live data. Those tests initially failed, and the tests were wrong, not
the collector: they held their own local `Rc`, which **is** a legitimate
external reference. They now observe through `Weak` after the handle dies —
which is also the exact shape of the leak.

### The cost, and where it falls

| | Δ |
| --- | --- |
| geomean | **+1.6%** |
| `bintree` | +7.6% |
| `fib` | +5.6% |
| `closure` | +3.3% |
| everything else | ≤ +2% |

`bintree` is real work: it stores instances into instance fields constantly,
so it registers constantly. Isolated by building without the VM's `SETFIELD`
hook — +7.8% becomes +1.6%. Raising the collection threshold 16x (8k → 128k)
only recovered 1.8 points, so the cost is the per-store `Rc::downgrade` and
push, not the scans; the low threshold is kept because it collects sooner
for nearly the same price.

`fib` and `closure` store no containers at all and were unaffected by
removing the VM hook, so their few percent is whole-binary code layout
(`lto = "fat"`, `codegen-units = 1`) — the same sensitivity recorded
throughout this file. `inline(never)` on `on_store` was tried against it and
made every row *worse*.

**Whether +1.6% is worth an unbounded leak going away is a product call, not
a performance one.** If it should be opt-out, the hook is three call sites
and a feature flag.

### What is still leaked

Cycles routed through an opaque variant — a closure capturing the thing that
holds it, a shared enum payload. Extending coverage means proving each new
edge cannot double-count one `Rc` between two parents; do not widen the
traversal without that argument.

### The original plan, for reference

**`Rc` cannot collect cycles, and Saule leaks them today.** 200,000 tables
in a loop, release build, VM engine:

```
t["pad"] = "…"                 →   4 MB peak RSS
t["self"] = t;  t["pad"] = "…" →  50 MB peak RSS
```

Unbounded, and it is the user's `t.self = t` that does it — not an exotic
shape. Any doubly-linked structure, any parent↔child instance graph, any
closure that captures the thing holding it. For a game loop or a long-lived
script this is a defect rather than a benchmark number.

This is already known here in the specific: `Closure::shared` is `Weak` on
purpose to break one cycle, and `closure_semantics.rs` guards another. Those
are hand-patched instances of a general hole.

### Do trial deletion on top of `Rc`, not a tracing GC

Bacon–Rajan trial deletion, the approach CPython uses:

- Only the container variants can form a cycle — `Table`, `Instance`,
  `EnumVariant`, and `Closure`/upvalues. `Str`, `Int`, `Float`, `Bool` and
  the native handles cannot, so most of `Value` is untouched.
- Buffer "candidate roots": objects whose refcount is decremented to
  something still non-zero.
- Periodically, over *only* those candidates, decrement internal references
  to find subgraphs unreachable from outside, then free them.

**Why this shape and not a real collector.** Two things make a tracing GC
expensive here specifically, and trial deletion sidesteps both:

1. **The native ABI.** `saule-native-abi`, `saule-sdk` and the
   `libloading` dynamic packages hand `Value`s to foreign code. Refcounting
   keeps those alive for free. Tracing would need every native to root what
   it holds — a breaking ABI change for every package, and an API whose
   misuse produces use-after-free rather than a clean error. Trial deletion
   changes nothing: natives keep their `Rc` and stay correct.
2. **The tree-walker's roots live on the Rust stack.** `eval` calls `exec`
   calls `eval`, recursively, with `Value`s in Rust locals that a precise
   collector cannot see — so tracing would need a shadow stack (invasive
   and slow, in the engine that is the differential oracle) or conservative
   stack scanning (fragile). Trial deletion needs neither: those locals are
   real strong references and stay counted.

No safepoints, no rooting, no ABI change. It is additive.

### What it is *not* for

**Not a performance task.** Refcount traffic is ~6% of `fib`
(`drop_in_place<Value>` 3.8% + `drop_in_place<Frame>` 2.4%), and `fib`'s
loop holds nothing but integers, which are not refcounted at all. A
collector would not move `fib` or `closure` — the two rows furthest from
Lua — measurably. Anyone reaching for GC to close the speed gap is reading
the wrong profile; see Task 5.

---

## Task 5 — Dispatch ✗ CLOSED: `execute_loop`'s size budget is already spent

**Tried the low-risk half first — a superinstruction, per §16's own
workflow — and it lost on precisely the benchmarks it targeted.**

`--profile-bytecode` gave an unusually clean case for fusing `ADDI RET1`:

```
fib      ADDI RET1   514,228 pairs   10.0% of all instructions executed
closure  ADDI RET1 1,000,000 pairs   12.5% of all instructions executed
```

In `closure` that pair is the *entire* body of the lambda, so a call there
cost two dispatches for one addition:

```
proto[0] <lambda>(2 params)      →   proto[0] <lambda>(2 params)
  0000  ADDI  2  0  1                  0000  ADDRET  2  0  1
  0001  RET1  2  0  0
```

It worked, and all six verification modes passed. Then:

| | `fib` | `closure` | geomean |
| --- | --- | --- | --- |
| `ADDRET` superinstruction | **+5.3%** | **+10.4%** | **+1.4%** |

*(min of 15, interleaved; confirmed across two sweeps.)*

**Removing 10-12.5% of the dispatches made those programs 5-10% slower.**
The fused arm cannot be outlined the way `Vm::concat` was — it ends in
`continue 'reentry`, so it is control flow that has to live inside the loop —
and one more inline arm costs this function more than a tenth of its
dispatches are worth.

### What that means for replication, without building it

The original plan here was to replicate the decode-and-jump into hot arms,
Lua-style, so each opcode gets its own branch-predictor entry. That plan
made `execute_loop` **much** bigger. This result prices that: if *one* added
arm costs `closure` 10%, replicating dispatch across the hot arms is not a
close call. Not attempted, and it should not be attempted without first
breaking the size dependency.

### The pattern this completes

Every dispatch-bound result in this file points the same way, and it is the
opposite of the intuition:

| change | direction | result |
| --- | --- | --- |
| profiling loop's *second copy* merely existing | bigger | -2-3% |
| `Vec<String>` removed from `CONCAT` | smaller | double digits on benchmarks that never concatenate |
| four allocating arms outlined | smaller | **-4.8% geomean, every row improved** |
| `ADDRET` arm added | bigger | **+1.4% geomean, worst on its targets** |

**`execute_loop` has a size budget and it is saturated.** The lever that
works is making the function smaller, not doing less work inside it. The
next person should audit the remaining arms for anything outlinable — the
four already moved were the ones that allocate, but they will not be the
only ones — rather than adding superinstructions, replicating dispatch, or
fusing pairs, all of which push the wrong way.

> **Amended once, by `set_reg`.** The table above is a real pattern but it
> was read too strongly. `set_reg` made `execute_loop` *bigger* (30 888 →
> 31 140 bytes) and `loop_arith` 25% faster, because what it removed was a
> **call** on the hot path, not work in general. So the rule is narrower
> than "smaller is better": size costs when it displaces hot arms, and a
> call costs more than the bytes that replace it. Both levers are real, and
> a change that trades a few bytes for a removed call is worth measuring
> rather than dismissing on the size argument.
>
> It also says something about method: this one was invisible to `sample`,
> which attributed the call to the arm that made it. Disassembling the hot
> arms found in an afternoon what profiling had not in a whole file of
> tasks. Read the machine code before concluding an arm is at its floor.

The corollary is that Saule's dispatch is at its architecture's floor.
Getting past it means a design where the hot loop is not one enormous
function — a tail-called handler table (`become`, RFC 3407, still unstable)
or generated code — not another change inside this one.

### The original plan, for reference

**Targets `fib` (2.43x) and `closure` (2.00x)** — the two rows Tasks 1-3 did
not move, and the ones a collector will not move either.

Lua dispatches with computed goto: the decode-and-jump is **replicated at
the end of every opcode handler**, so each opcode gets its own
branch-predictor entry. Rust's `match` compiles to a single indirect branch
shared by all ~100 opcodes, which predicts poorly on a varied instruction
stream. On a tight loop that is a structural disadvantage, and it is the
most plausible remaining explanation for `fib`.

Stable Rust cannot express guaranteed tail calls (`become`, RFC 3407, is
unstable), so the approximation is to duplicate the decode-and-dispatch tail
into the hottest arms so they branch straight to their successor rather than
back to the shared head.

### Handle with the care this loop has earned

`execute_loop` is dominated by its own code size — recorded three times over
in this file and in `Cargo.toml`. Replicating dispatch makes the function
substantially *bigger*, which is exactly the axis that has produced both the
largest wins and the largest regressions of this whole effort. It could pay
double digits or cost double digits.

So: replicate into a **small** set of arms first — the ones
`--profile-bytecode` shows carrying `fib` and `closure` — measure against
the whole suite, and widen only while the geomean keeps improving. Do not
convert all hundred arms and then measure.

---

## Expected outcome, and the ceiling

Task 1 has landed and moved the string and table half decisively (`map` is
now **0.46x** Lua). Task 3's remaining items are small. Task 2 is closed as
a loss.

Task 5 is now closed too, and it answers the question it was written to ask:
this VM **is** at its design's floor. `fib` and `closure` are held back by
dispatch, and dispatch cannot be improved from inside `execute_loop`, because
every change that adds code to that function costs more than the work it
saves. Going further means a different architecture for the hot loop — a
tail-called handler table, or generated code — not another optimization
within this one.

So the standing recommendation for speed is narrow: **audit the remaining
arms for anything that can be moved out of line.** That is the only lever
with a track record here (-4.8% geomean, every row improved), and the four
arms already outlined were chosen for allocating, not for being the only
candidates.

Task 4 has landed and is orthogonal to all of it: it buys no speed, costs
+1.6%, and makes an unbounded leak bounded.
