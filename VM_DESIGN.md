# Saule Bytecode VM — Design and Implementation Plan

> **Status:** in progress. `crates/saule-vm` exists and runs — the instruction
> encoding, chunk model, disassembler, and the core dispatch loop are
> implemented (§21.2 and most of §21.3's runtime half). There is no compiler
> yet, and Phase 0 is untouched. **`VM_TASKS.md` is the live checklist**;
> this document remains the specification.
> **Audience:** anyone working on `crates/saule-vm`, `crates/saule-interpreter`,
> `crates/saule-typeck`, or `crates/saule-semantic`.
> **Companion documents:** `PRODUCTION.md` §"How fast is it?" and Appendix A for
> the measurements this design is arguing against; `README.md` for language
> semantics the VM must preserve exactly.

---

## Table of Contents

**Part I — Why**
1. [Where the time goes today](#1-where-the-time-goes-today)
2. [The structural advantage Saule has over Lua](#2-the-structural-advantage-saule-has-over-lua)
3. [Design principles](#3-design-principles)

**Part II — The machine**
4. [Value representation](#4-value-representation)
5. [Chunks, protos, and instruction encoding](#5-chunks-protos-and-instruction-encoding)
6. [The register stack and calling convention](#6-the-register-stack-and-calling-convention)
7. [Closures and upvalues](#7-closures-and-upvalues)
8. [Classes: layout, vtables, interfaces](#8-classes-layout-vtables-interfaces)
9. [Enums and `match`](#9-enums-and-match)
10. [Tables](#10-tables)
11. [Loops and iteration](#11-loops-and-iteration)
12. [Errors, `throw`/`catch`, and spans](#12-errors-throwcatch-and-spans)
13. [Natives and the stdlib boundary](#13-natives-and-the-stdlib-boundary)
14. [Modules and imports](#14-modules-and-imports)

**Part III — The instruction set**
15. [Complete opcode reference](#15-complete-opcode-reference)
16. [Superinstructions and specialization](#16-superinstructions-and-specialization)

**Part IV — The compiler**
17. [Compiler passes](#17-compiler-passes)
18. [Register allocation](#18-register-allocation)
19. [Compile-time argument binding](#19-compile-time-argument-binding)

**Part V — Getting there**
20. [Expected performance](#20-expected-performance)
21. [Implementation plan](#21-implementation-plan)
22. [Keeping the tree-walker alive](#22-keeping-the-tree-walker-alive)
23. [Testing strategy](#23-testing-strategy)
24. [Risks and open questions](#24-risks-and-open-questions)
25. [Appendix A — file-by-file change map](#appendix-a--file-by-file-change-map)
26. [Appendix B — worked compilation examples](#appendix-b--worked-compilation-examples)

---

# Part I — Why

## 1. Where the time goes today

`PRODUCTION.md` Appendix A puts Saule at 5–11× slower than PUC Lua, with the
worst cases being `fib` (10.9×) and `loop_arith` (7.0×). That is not a diffuse
"tree-walking is slow" cost. It is four specific, identifiable structures.

### 1.1 Name lookup is a hash-map chain walk

`crates/saule-interpreter/src/env.rs:22`

```rust
pub struct Environment {
    parent: Option<Rc<RefCell<Environment>>>,
    vars: HashMap<Rc<str>, Value>,
    cells: HashMap<Rc<str>, Rc<RefCell<Value>>>,
    statics_owner: Option<Rc<ClassObject>>,
    module_dir: Option<PathBuf>,
    loader: Option<Rc<RefCell<ModuleLoader>>>,
}
```

Every `Expr::Ident` in `eval/expr/mod.rs:73` becomes `env.borrow().get(name)`,
which is: a `RefCell` borrow, an FxHash of the string, a bucket probe, and on a
miss the same again one scope up — repeated to the module root. A variable read
inside a nested loop inside a method can easily be four or five probes deep.

The comment at `env.rs:30-41` documents a real measurement: wrapping bindings in
a `Direct | Cell` enum cost 3–8%. That is the scale of the margins available
*inside* this design. A VM does not shave the map lookup — it deletes it. `R[3]`
is an index into a `Vec<Value>`.

### 1.2 Scope allocation per block and per call

`Environment::with_parent` (`env.rs:76`) pulls from a 128-entry thread-local
pool, and `Environment::recycle` (`env.rs:143`) reuses a loop body's scope when
nothing captured it. Both are careful, well-measured optimizations. Both exist
only because the design allocates a scope object where a VM increments a base
pointer.

`block_binds_names` (`eval/stmt/mod.rs:104`) skips the scope entirely when a
block declares nothing — again, real engineering against a structural cost.

### 1.3 Argument binding is a runtime search

`bind_params` in `eval/expr/calls/binding.rs` runs on **every call**. Even the
documented "fast path" (`binding.rs:182-218`) does, per call:

- a scan over `params` looking for a variadic (`binding.rs:151`),
- a scan over `args` for any `Named` (`binding.rs:183`),
- a scan over `args` for any `TrailingBlock`,
- then a per-parameter loop doing `scope.borrow_mut().define(...)` — a hash
  insert per parameter.

The general path allocates `Vec<Option<Value>>` and does an O(params) `position`
lookup per named argument.

**Every bit of this is statically known.** The callee's parameter list, the
argument list, which slot each argument fills, which defaults fire — all of it is
determined by the source text. `saule_ast::resolve_arg_slots` (`expr.rs:327`)
already computes exactly this mapping; the typechecker already calls it. It is
being recomputed at runtime because there is nowhere to cache the answer.

### 1.4 Statically-known operations dispatched dynamically

- `ops::binary` (`eval/ops.rs:120`) re-discovers on every `+` whether the operands
  are two ints, two floats, an overload, or an error. The typechecker already
  proved which.
- `read_member` (`eval/expr/members.rs:50`) hashes a `&str` against
  `InstanceObject.fields: HashMap<String, Value>` — a **per-instance hash map**.
  Every instance of `Vec2` carries its own map with `"x"` and `"y"` in it.
- `ClassObject::lookup_method` (`value/class.rs:52`) walks the parent chain,
  hashing the method name once per level.

### 1.5 Every instance carries a hash map

`value/class.rs:45`:

```rust
pub struct InstanceObject {
    pub class: Rc<ClassObject>,
    pub fields: HashMap<String, Value>,
}
```

For a `Vec2 { x, y }` this is ~48 bytes of `Value` payload wrapped in a hash
table with a bucket array, control bytes, and `String` keys that were cloned at
construction time (`eval/expr/construct.rs:122`:
`inst.borrow_mut().fields.insert(field.name.clone(), value)` — a `String`
allocation per field per instance). This is why `oop` is 7.6× Lua.

---

## 2. The structural advantage Saule has over Lua

This is the most important point in the document, and the reason the target is
*beating* PUC Lua rather than approaching it.

A Lua VM cannot know, at compile time:

- whether `a + b` is integer, float, string-coercion, or a `__add` metamethod;
- where in a table `t.field` lives, or whether a metatable `__index` chain
  intervenes;
- what `obj:method()` resolves to.

Lua's bytecode therefore encodes *dynamic* operations and pays tag checks and
hash lookups at runtime. LuaJIT's entire advantage comes from speculatively
removing those checks at runtime with a tracing JIT.

Saule already knows all three statically:

| Question | Who answers it today | Where |
|---|---|---|
| Is `a + b` int, float, or an `Op*` overload? | `saule-typeck` | `crates/saule-typeck/src/ops.rs`, `expr/operators.rs` |
| What class is `obj`, and does it have field `f`? | `saule-semantic` registry | `crates/saule-semantic/src/registry.rs` — `lookup_member`, `lookup_field_type` |
| What does `obj.method(...)` resolve to? | `saule-semantic` | `registry.rs` — `lookup_method` |
| Which parameter slot does each argument fill? | `saule-ast` | `expr.rs:327` — `resolve_arg_slots` |
| Is a class a subtype of another? | `saule-semantic` | `is_subtype_named` |

Classes are **nominal** with **single inheritance** (`Decl::Class.extends` is
`Option<String>`, `decl.rs:30`). Field offsets and vtables are therefore
computable, not speculative. Integers and floats are separate types that
`saule-typeck` refuses to mix (`ops.rs:302` — `RuntimeError::NumericMix` exists
only for the unchecked `run()` entry point).

**Today all of this is thrown away.** `saule_typeck::check` returns
`Vec<TypeCheckError>` and nothing else (`crates/saule-typeck/src/lib.rs:56`).
`saule_semantic::analyze` returns `Vec<SemanticError>` (`lib.rs:91`). The
inference results are computed, used for diagnostics, and dropped.

**Recovering that information is the prerequisite for everything else in this
document.** A VM built without it lands around 2.5× faster than the tree-walker.
A VM built with it lands around 6×.

---

## 3. Design principles

1. **Preserve semantics exactly.** `tests/*.sau` and `tests/ui/*.sau` are the
   contract. The VM is an implementation change, not a language change.
2. **The tree-walker keeps working at every commit.** See §22. Every
   preparatory change must be independently justifiable as an improvement to
   the existing interpreter.
3. **Static knowledge is compiled in, never rediscovered.** If the typechecker
   proved it, the opcode encodes it.
4. **Specialize the common shape, keep a general fallback.** Every typed opcode
   has an `any`-typed sibling. Correctness never depends on the specialization
   firing.
5. **Optimize the memory hierarchy before the dispatch loop.** Cache misses and
   refcount traffic dominate; instruction dispatch is maybe 15% of the budget.
6. **No JIT.** Out of scope, permanently, for this document.
7. **Reuse the existing runtime.** `Value`, `TableObject`, the stdlib, the
   native ABI, and the diagnostic machinery are good. The VM replaces execution,
   not the runtime.

---

# Part II — The machine

## 4. Value representation

### 4.1 Keep the existing enum

`crates/saule-interpreter/src/value/mod.rs:38`. Measured size on 64-bit:

```
Value         = 16 bytes, align 8
Option<Value> = 16 bytes   (niche in the discriminant)
```

No variant payload exceeds 8 bytes, so the enum is one word of payload plus a
tag word. **This is already competitive with NaN-boxing.**

### 4.2 Why not NaN-box

NaN-boxing packs a value into 8 bytes by hiding pointers in the unused bit
patterns of a quiet NaN. It would halve register-stack footprint. It would not
help nearly as much as it does in a dynamic language, because:

- **The dominant cost is refcount traffic, not size.** Every `Value` clone that
  holds an `Rc` is a non-atomic increment plus a potential cache-line write to a
  shared control block. NaN-boxing does not remove a single one.
- **`i64` does not fit.** Saule's `integer` is a full 64-bit signed value
  (`README.md`, "Types"). NaN-boxing gives you 51 bits of payload. Lua 5.4
  abandoned NaN-boxing for exactly this reason when it introduced a distinct
  integer type. Saule has the same constraint, and Saule's integers are *more*
  load-bearing than Lua's.
- **It infects everything.** Every `match` on `Value` across ~13k lines of
  interpreter and stdlib becomes accessor calls.

**Decision: keep `Value` as-is.** Revisit only after §21 Phase 5, with
measurements.

### 4.3 What to attack instead: refcount traffic

Concrete mitigations, in priority order:

1. **Arithmetic never touches refcounts.** `ADDI R[a], R[b], R[c]` reads two
   `i64`s out of the register file and writes an `i64`. No `Rc`, no clone. Today
   `ops::binary` takes `Value` by value, which means the caller cloned.
2. **Arguments are constructed in place.** The compiler emits argument
   evaluation *directly into* the callee's future register window (§6.2), so a
   call copies nothing.
3. **Reads borrow.** The VM loop indexes `&self.stack[base + b]` and clones only
   when storing.
4. **`MOVE` on a dead source could become a move.** Deferred — the liveness
   analysis is not worth it until measured.

### 4.4 Additive runtime changes

Two small changes to existing types, both independently useful (§21 Phase 0):

**`InstanceObject` becomes slot-based** (`value/class.rs:45`):

```rust
pub struct InstanceObject {
    pub class: Rc<ClassObject>,
    pub fields: Vec<Value>,          // was HashMap<String, Value>
}

pub struct FieldLayout {
    pub index: HashMap<Rc<str>, u16>, // name -> slot, shared per class
    pub names: Vec<Rc<str>>,          // slot -> name, for diagnostics/display
}
// on ClassObject:
pub layout: Rc<FieldLayout>,
```

Parent fields occupy the low slots so a subclass's layout is a prefix-compatible
extension of its parent's — which is what makes a `GETF` compiled against the
static type valid for any subclass instance.

**`EnumVariantObject` gets a dense tag** (`value/enum_.rs:10`):

```rust
pub struct EnumVariantObject {
    pub enum_name: String,
    pub variant_name: String,
    pub tag: u32,                     // NEW: dense index, 0..variant_count
    pub value: Option<Value>,
    pub enum_obj: RefCell<Option<Rc<EnumObject>>>,
}
// on EnumObject:
pub by_tag: Vec<Rc<EnumVariantObject>>,
```

Both changes speed up the tree-walker on their own. See §21.1.

---

## 5. Chunks, protos, and instruction encoding

### 5.1 Data model

```rust
/// One compiled module.
pub struct Chunk {
    pub protos:     Vec<Proto>,        // every function/method/lambda in the module
    pub classes:    Vec<ClassProto>,
    pub enums:      Vec<EnumProto>,
    pub constants:  Vec<Value>,        // module-wide, deduplicated
    pub module_slots: usize,           // top-level bindings, flat
    pub main:       ProtoIdx,          // the module body
    pub source:     Rc<miette::NamedSource<String>>,
}

pub struct Proto {
    pub name:        Option<Rc<str>>,
    pub n_params:    u8,
    pub is_variadic: bool,
    pub max_regs:    u8,               // frame size
    pub code:        Vec<Instruction>, // Vec<u32>
    pub upvals:      Vec<UpvalDesc>,
    pub protos:      Vec<ProtoIdx>,    // nested closures
    pub handlers:    Vec<Handler>,     // try/catch ranges, §12
    pub lines:       Vec<LineEntry>,   // pc -> span, §12.3, cold
    pub caches:      Vec<InlineCache>, // §8.5
    pub owner_class: Option<ClassIdx>, // for `self.super` and statics
}

pub struct UpvalDesc {
    pub from_parent_stack: bool,       // true: parent register, false: parent upvalue
    pub index: u8,
    pub name: Rc<str>,                 // diagnostics only
}
```

`Proto` is the direct replacement for `FunctionObject`'s AST body
(`value/function.rs:147` — `FunctionBody::Block(Arc<[Spanned<Stmt>]>)`).

The runtime closure:

```rust
pub struct Closure {
    pub proto: Rc<Proto>,
    pub upvals: Vec<Rc<RefCell<Upvalue>>>,
}
```

### 5.2 Instruction encoding

Fixed-width 32-bit words. Decoding is shifts and masks; no unaligned access, no
variable-length instruction pointer arithmetic.

```
 31       24 23       16 15        8 7         0
┌───────────┬───────────┬───────────┬───────────┐
│    op     │     A     │     B     │     C     │   ABC
├───────────┼───────────┼───────────┴───────────┤
│    op     │     A     │          Bx           │   ABx   (unsigned 16)
├───────────┼───────────┼───────────────────────┤
│    op     │     A     │         sBx           │   AsBx  (signed 16, biased)
├───────────┼───────────┴───────────────────────┤
│    op     │              Ax                   │   Ax    (unsigned 24)
└───────────┴───────────────────────────────────┘
```

Notation used throughout §15:

| Symbol | Meaning |
|---|---|
| `R[n]` | register `n` of the current frame — `stack[base + n]` |
| `K[n]` | constant `n` of the enclosing chunk |
| `U[n]` | upvalue `n` of the current closure |
| `M[n]` | module slot `n` |
| `P[n]` | nested proto `n` of the current proto |
| `top` | the frame's current stack top (only meaningful at variadic points) |

Consequences of 8-bit `A`/`B`/`C`:

- **256 registers per function.** Lua has lived with 255 for 30 years. The
  compiler must emit a clear diagnostic ("function too complex") if a body
  exceeds it; in practice only machine-generated code will.
- **Operands needing more than 8 bits use a following `EXTRAARG` word**
  (24 bits). Used by `CALLM_MR`, `CALLIF`, and large-index forms. The dispatch
  loop reads it inline: `let extra = code[pc]; pc += 1;`.
- **Jumps are ±32767 instructions.** Beyond that, the compiler emits a trampoline
  (`JMP` to a `JMP`). Vanishingly rare; must not be a panic.

### 5.3 Dispatch loop

```rust
loop {
    let ins = unsafe { *code.get_unchecked(pc) };  // pc bounds-proven by the verifier
    pc += 1;
    match op_of(ins) {
        Op::MOVE => { ... }
        Op::ADDI => { ... }
        // ...
    }
}
```

A dense `#[repr(u8)]` opcode enum in a `match` compiles to a jump table under
LLVM. **Do not** start with computed-goto emulation, function-pointer threading,
or `become`-based tail dispatch. Those are worth 5–15% and cost a great deal of
readability; they belong in Phase 5 with a benchmark attached.

The one thing worth doing early: keep `pc`, `base`, and a raw `*const
Instruction` in locals across the loop rather than reloading from the frame
struct. Reload them only after a `CALL`.

---

## 6. The register stack and calling convention

### 6.1 Stack layout

```rust
pub struct Vm {
    stack:  Vec<Value>,     // one contiguous register file, grown, never shrunk
    frames: Vec<Frame>,
    open_upvals: Vec<Rc<RefCell<Upvalue>>>,  // sorted by stack index, §7
    modules: Vec<Value>,    // flat module slots
}

struct Frame {
    closure: Rc<Closure>,
    base:    u32,   // R[0] == stack[base]
    ret_to:  u32,   // absolute register where results go in the caller
    n_ret:   u8,    // how many results the caller wants; 255 = all
    pc:      u32,   // saved program counter
    top:     u32,   // for variadic call/return points
}
```

`Environment` disappears entirely. A call is `frames.push(Frame { .. })` plus a
bounds check that `stack.len() >= base + proto.max_regs`.

### 6.2 Calling convention

Lua's convention, adopted because it is correct and it eliminates argument
copying:

```
              ┌─────────┬─────────┬─────────┬─────────┐
caller frame  │  R[A]   │ R[A+1]  │ R[A+2]  │ R[A+3]  │
              │ callee  │  arg 0  │  arg 1  │  arg 2  │
              └─────────┴─────────┴─────────┴─────────┘
                             ▲
                             └── callee's base; arg i is callee's R[i]
```

The compiler evaluates the callee into `R[A]` and each argument directly into
`R[A+1+i]`. `CALL A B C` then sets `base = A + 1`. **Arguments are never copied
— they are already in the callee's frame.**

- `B = nargs + 1`; `B = 0` means "arguments run from `A+1` to `top`", used when
  the last argument is itself a multi-value call (`f(g())`).
- `C = nret + 1`; `C = 0` means "all results, set `top`".
- On return, results are copied from the callee's frame down to `R[A]..` in the
  caller.

For **methods**, the receiver occupies `R[A]` and becomes the callee's `R[0]`,
i.e. `self` is simply parameter 0. This exactly matches the existing convention
where `user_params` skips a leading `self` (`eval/expr/calls/binding.rs`, and
`FunctionObject::user_param_keys` at `value/function.rs:107`).

### 6.3 Multi-return

Saule is multi-return (`Flow::Return(Vec<Value>)` at `eval/mod.rs:108`, and
`eval_values` at `eval/expr/calls/invoke.rs:196`). In the VM this becomes a
register range, and the entire `recycle.rs` free-list machinery — which exists
*only* to stop `Vec<Value>` and `Vec<EvaluatedArg>` allocations per call — is
deleted.

`RET A B` returns `R[A]..R[A+B-2]`; `B = 0` returns `R[A]..top`.

### 6.4 Recursion depth

`MAX_EVAL_DEPTH = 10_000` (`eval/mod.rs:35`) exists because the tree-walker
recurses on the **native** stack, and a native stack overflow is `SIGSEGV`, not a
catchable panic. The doc comment there is explicit about it.

In the VM, a Saule call is a `Vec` push. The limit becomes a policy on
`frames.len()` and can be raised by two orders of magnitude. `SAULE_MAX_DEPTH`
keeps working; its meaning becomes "frames", which is what users assumed anyway.

This also makes **`TAILCALL` implementable**, closing the gap `PRODUCTION.md:344`
identifies ("No tail calls, so recursive-loop idioms from Lua will hit the depth
limit").

---

## 7. Closures and upvalues

### 7.1 What replaces flat capture

`crates/saule-interpreter/src/capture.rs` computes, for a lambda body, "every
identifier the body mentions". Its own doc comment (`capture.rs:17-40`) is honest
that this is a deliberate over-approximation, and that it **bails out entirely**
(`opaque = true`, returns `None`) on a nested `Stmt::Decl`, falling back to
whole-scope capture — the leaking behaviour.

`Environment::capture_flat` (`env.rs:317`) then promotes each mentioned name into
an `Rc<RefCell<Value>>` cell shared between the defining scope and the closure.

**The VM replaces this with Lua-style open/closed upvalues**, which is the exact,
non-approximating version of the same idea:

- An **open** upvalue points into the register stack at an absolute index. Reads
  and writes go straight to the live register — the closure and the enclosing
  frame observe each other, which is the live-binding semantics `env.rs:317`'s
  doc block is protecting.
- When the enclosing scope exits, `CLOSEUP` **closes** every open upvalue at or
  above a given register: the value is moved out of the stack into the cell.

```rust
pub enum Upvalue {
    Open(u32),      // absolute stack index
    Closed(Value),
}
```

`UpvalDesc` on the proto says where each upvalue comes from when the closure is
built: parent register `n`, or parent upvalue `n`. The compiler computes this
exactly, from real free-variable analysis — no bail-out, no over-approximation.

### 7.2 Per-iteration capture

`Environment::recycle` (`env.rs:143`) gives a loop body a fresh scope per
iteration *only when something captured the previous one*, which is what makes
this work:

```saule
for i = 1 to 3 do
  fns[i] = fn() return i end   -- three distinct `i`s
end
```

In the VM this is `CLOSEUP` at the bottom of the loop body, targeting the first
register the body owns. Closing converts each iteration's open upvalue to a
`Closed(Value)` holding that iteration's value, and the next iteration reuses the
register with a fresh open upvalue. Same semantics, no allocation when nothing
captures, and no `strong_count` probe.

### 7.3 Self-recursive local closures

`FunctionObject::self_name` (`value/function.rs:92`) plus
`Environment::drop_capture` (`env.rs:350`) exist to break the cycle in
`local fact = fn(n) … fact(n-1) … end`: the recursive reference is re-bound per
call rather than captured, because capturing it would close a
cell → function → scope → cell loop.

The VM handles this structurally. `local fact = fn ... end` compiles to:

```
LOADNIL  r5 0          ; reserve the slot
CLOSURE  r5 P[0]       ; upvalue 0 of P[0] points at parent register r5
```

The closure holds an **open** upvalue into `r5`. Reading it reads the stack
slot — which by then holds the closure. There is no `Rc` cycle at all while the
upvalue is open. When the scope exits, `CLOSEUP r5` closes it, and *that* does
create a cycle (closure → cell → closure). This is the one place a cycle is
genuinely unavoidable without a tracing collector; it is exactly as leaky as
today's behaviour and no worse. See §24.3.

---

## 8. Classes: layout, vtables, interfaces

This is where the largest win is available (`oop`: 7.6× Lua today).

### 8.1 ClassProto

```rust
pub struct ClassProto {
    pub name: Rc<str>,
    pub parent: Option<ClassIdx>,
    pub layout: Rc<FieldLayout>,        // §4.4 — parent fields first
    pub field_template: Option<Vec<Value>>,  // Some when all defaults are constant
    pub field_init: Option<ProtoIdx>,        // synthetic proto for non-constant defaults
    pub vtable: Vec<Rc<Closure>>,       // instance methods, parent slots inherited
    pub vindex: HashMap<Rc<str>, u16>,  // name -> vtable slot (compile-time use)
    pub statics: Vec<Value>,            // flat, RefCell at the Class value level
    pub sindex: HashMap<Rc<str>, u16>,
    pub static_methods: Vec<Rc<Closure>>,
    pub itables: HashMap<InterfaceIdx, Vec<u16>>,  // iface slot -> vtable slot
    pub init: Option<u16>,              // vtable slot of `init`, resolved through the chain
}
```

### 8.2 Field access

Layout rule: **a subclass's field slots are a prefix-extension of its parent's.**
`init_fields` (`eval/expr/construct.rs:97`) already initializes parent fields
before child fields; the layout formalizes that ordering.

Given `local p: Player = ...`, the compiler knows `p`'s static type is `Player`,
looks up `layout.index["health"]` at compile time, and emits:

```
GETF  r3 r1 4        ; r3 := r1.fields[4]
```

One indexed load out of a `Vec<Value>`. No hash, no string, no chain walk. And
because layouts are prefix-compatible, the same instruction is correct when `r1`
actually holds a `Warrior` that extends `Player`.

**Fallback.** When the receiver's static type is `any`, an interface, or
otherwise unknown, emit:

```
GETFX r3 r1 Kx       ; + inline cache slot in EXTRAARG
```

which checks a one-entry cache (`class ptr -> slot`), and on a miss does the
name lookup and refills it. Method-call sites on interface receivers are
overwhelmingly monomorphic, so the cache hits.

### 8.3 Method dispatch

`ClassObject::lookup_method` (`value/class.rs:52`) walks the parent chain hashing
the name at each level. The VM flattens it: each class's `vtable` starts as a
copy of its parent's, with overrides written in place and new methods appended.
Dispatch is then one indexed load.

```
CALLM  r2 3 7        ; r2 = receiver, 2 args in r3..r4, vtable slot 7, 1 result -> r2
```

Because inheritance is single and nominal, a subclass's vtable is a
prefix-extension of its parent's, exactly like the field layout. A `CALLM`
compiled against the static type `Player` remains correct for a `Warrior`
receiver and dispatches to the override. This is C++-style virtual dispatch and
it is sound here for the same reasons.

### 8.4 Interfaces

An interface has no layout of its own; a class implements many. Per class, build
`itables: InterfaceIdx -> Vec<u16>` mapping the interface's method slot to the
class's vtable slot. Then:

```
CALLIF r2 3 5        ; + EXTRAARG = interface index
```

is: read the receiver's class, one small-map probe, one vtable index. Add a
one-entry inline cache keyed on the class pointer and the common monomorphic
case collapses to a pointer compare.

### 8.5 Inline caches

```rust
pub enum InlineCache {
    Empty,
    Mono { class: *const ClassObject, slot: u16 },
}
```

Stored in `Proto.caches`, indexed by an operand. Invalidation is unnecessary:
class layouts are immutable once built (there is no runtime class mutation in
Saule — no metatables, no monkey-patching), so a cached `(class, slot)` pair is
permanently valid.

### 8.6 Construction

`NEW A Bx` — allocate an instance of `ClassProto[Bx]`:

1. If `field_template` is `Some`, clone the template `Vec<Value>`. This is one
   allocation and a memcpy of 16-byte values, replacing today's per-field
   `String` clone plus hash insert (`construct.rs:122`).
2. Otherwise call `field_init` (a synthetic proto that runs the parent's first,
   mirroring `init_fields`'s recursion) with `self` in `R[0]`.

Then the constructor is an ordinary `CALLM` on the `init` slot, resolved through
the chain at compile time exactly as `constructor_chain` (`construct.rs:127`)
does at runtime.

### 8.7 Operator overloading

`ops::binary` (`eval/ops.rs:120`) gates the overload check on
`matches!(l, Value::Instance(_)) || matches!(r, Value::Instance(_))`, so the
common numeric path already skips it. In the VM the typechecker has already
decided:

- both operands `integer` → `ADDI`
- both `float` → `ADDF`
- an operand is a class implementing `OpAdd` → `CALLM` on the `add` slot,
  emitted directly by the compiler
- operand is `any` → `ADDX`, the fully dynamic form, which contains today's
  `ops::binary` logic verbatim

`saule_ast::ops::binary_contract` (`crates/saule-ast/src/ops.rs:90`) already
gives the compiler the method name for each operator. The dispatch-on-left-operand
rule and the `==`/`compare` symmetry rules (`eval/ops.rs:199-245`) move into the
compiler, where they cost nothing at runtime.

`..` needs care: `Concat` falls through to `OpToString` when the left operand
does not overload it (`eval/ops.rs:267`). Compile that as a call to the
`toString` slot followed by `CONCAT`.

---

## 9. Enums and `match`

### 9.1 Dense tags

Give every variant a dense `tag: u32` in declaration order (§4.4). Bare and
valued variants stay singletons — `EnumObject.variants` already caches them so
identity is stable (`value/enum_.rs:23`). Tuple variants construct fresh objects
carrying the same tag.

### 9.2 `match` becomes a jump table

`match` is a primary control structure in Saule and today it is a linear chain of
pattern tests (`eval/expr/match_.rs`). When every arm's pattern is a variant of
one enum — the dominant shape — compile to:

```
GETTAG   r4 r2          ; r4 := tag of the enum variant in r2
SWITCH   r4 Bx          ; Bx indexes a jump table in the chunk
```

O(1) instead of O(arms). Same treatment for a `match` over small integer
literals when the values are dense.

Mixed or guarded matches fall back to a test chain built from `JIFTAG`,
`JEQK`, and ordinary comparison-branches. Guards (`when <expr>`,
`MatchArm.guard` at `crates/saule-ast/src/expr.rs:135`) compile as a conditional
branch to the next arm after the pattern test succeeds.

### 9.3 Payload destructuring

`Pattern::Variant { fields }` binds positionally out of the payload table
(`construct.rs:44` builds it as `TableObject::from_array`). Compile to
`UNWRAP` + `GETARR` per bound field, straight into the arm's registers.

`Pattern::Tuple` destructures a multi-return scrutinee — the values are already
in adjacent registers, so this is free.

### 9.4 `match` as an expression

`Expr::Match` is an expression (`expr.rs:89`), and arms may contain `return`.
Today that escapes through a thread-local `pending_flow` slot plus a
`RuntimeError::PendingFlow` marker (`eval/stmt/mod.rs:58-72`) because an
expression evaluator cannot return a `Flow`.

**In the VM this entire mechanism disappears.** Every arm writes its value to the
same destination register and jumps to a common exit label; a `return` inside an
arm is simply a `RET` instruction. There is no expression/statement evaluator
split to bridge.

---

## 10. Tables

`TableObject` (`value/table.rs`) is a hybrid dense array plus a hash map, and
`PRODUCTION.md` Appendix A shows `map` at 1.2× Lua — the table implementation is
already good. **Do not rewrite it.** The VM's job is to stop paying for dynamic
dispatch on top of it.

| Situation | Opcode | Cost |
|---|---|---|
| `t[i]` where typeck proved `t: table<T>`, `i: integer` | `GETARR` | bounds check + indexed load |
| `t[k]` where typeck proved `t: table<K,V>` | `GETMAP` | one hash probe |
| `t[x]` where `t: any` | `GETIDX` | today's `read_index` logic |
| `t.name` on a map table | `GETMAPK` | hash probe with a precomputed constant-key hash |

Constant string keys should carry a **precomputed hash** in the constant pool, so
`t.name` never re-hashes `"name"`.

Table literals compile to `NEWT` (with array/map size hints taken from the
literal, so the `Vec` and the map are allocated once at the right capacity)
followed by `SETLIST` to bulk-move a register range into the array part —
replacing the per-entry `table.array.push(v)` loop at `eval/expr/mod.rs:203`.

`APPEND` handles `Table.insert`-style growth without a call.

---

## 11. Loops and iteration

### 11.1 Numeric `for`

`run_numeric_loop_int` (`eval/stmt/loops.rs:59`) allocates or recycles a scope
per iteration and does a hash insert per iteration to bind the loop variable.

The VM keeps the counter, limit, and step in three consecutive registers with the
user-visible loop variable in a fourth:

```
FORPREP  r4 →exit      ; r4=counter r5=limit r6=step r7=user variable
body:
  ...
FORLOOP  r4 →body
exit:
```

`FORPREP` validates the bounds once (both int or both float — the
`RuntimeError::NumericMix` / `ZeroStep` checks at `loops.rs:37-52` happen here,
once, not per iteration) and skips the loop entirely if it will not run.
`FORLOOP` does a `wrapping_add`, an overflow check (matching `loops.rs:84`), a
compare, and a branch. Separate `FORLOOP_I` / `FORLOOP_F` variants so there is
no tag check.

### 11.2 Generic `for … in`

`exec_for_in` (`loops.rs:117`) has two paths, and the VM mirrors both:

**Table path.** Today it snapshots `array` and a *sorted* `map_entries` vector
before iterating, so the table may mutate during the loop. That snapshot is
observable behaviour and must be preserved. `ITERPREP` performs it, storing the
snapshot in a control register; `ITERNEXT` walks array entries first, then sorted
map entries, writing key and value into the loop variables.

**Closure-driver path.** For functions and for instances (which must expose
`iter()` returning a function), `ITERPREP` resolves the driver — calling `iter()`
on an instance via `CALLM` — and `ITERNEXT` calls it, stopping on a `nil` first
result. Loop variables are bound positionally from the returned register range.

`ITERPREP` also carries the arity check (`loops.rs:164`: one or two variables) so
it fires once rather than per iteration.

---

## 12. Errors, `throw`/`catch`, and spans

### 12.1 Handler tables, not `Result` on the hot path

```rust
pub struct Handler {
    pub pc_start: u32,
    pub pc_end:   u32,     // exclusive
    pub target:   u32,     // catch block entry
    pub err_reg:  u8,      // register the caught value lands in
    pub catch_ty: TypeIdx, // for the runtime type test `try/catch` performs
}
```

A `throw` sets `vm.pending = value` and unwinds: for each frame from innermost
out, binary-search `handlers` for one whose range contains the saved `pc`. On a
match, restore the frame, close upvalues above `err_reg`, store the value, and
jump to `target`. If no frame handles it, convert to `RuntimeError::Thrown` and
return `Err` from the VM.

**This removes the `thrown_slot` thread-local hack** (`eval/stmt/mod.rs:36-52`),
which exists because `RuntimeError` must be `Send + Sync` for miette while the
thrown `Value` contains non-`Send` `Rc`s. Inside the VM the value never enters a
`RuntimeError` at all unless it escapes to the top level.

The happy path pays nothing: entering a `try` block emits **zero** instructions.

### 12.2 What still returns `Result`

The VM loop returns `Result<Vec<Value>, RuntimeError>` for genuine failures:
native-function errors, division by zero, force-unwrap of nil, stack overflow,
I/O. These are cold. `RuntimeError` (`error.rs`) is unchanged.

### 12.3 Spans

Diagnostics quality is a stated strength and must not regress. Per proto:

```rust
pub struct LineEntry { pub pc: u32, pub span_start: u32, pub span_end: u32 }
```

Sorted by `pc`; a runtime error binary-searches it. This is **out of band** — it
never touches the instruction stream, so it costs nothing until something fails.

`FunctionObject.source` (`value/function.rs:81`) — the per-module `NamedSource`
that lets an error inside an imported module render against the right file —
moves onto `Proto`, and `attach_module_source` (`eval/expr/calls/invoke.rs:130`)
works unchanged.

An optional `--emit-line-table=none` mode can drop it for embedded builds.

---

## 13. Natives and the stdlib boundary

**None of `crates/saule-interpreter/src/stdlib/` needs to change.** That is ~3500
lines of the crate and the whole native-package system (`native_packages.rs`,
`dynamic_packages/`, `saule-native-abi`, `saule-sdk`) hanging off it.

`NativeFn` (`value/function.rs:20`):

```rust
pub struct NativeFn {
    pub name: &'static str,
    pub func: fn(&[Value]) -> Result<Value, String>,
}
```

The VM passes `&self.stack[base..base + nargs]` — a borrow of the register file,
zero copies. This is *better* than today, where `call_value_multi`
(`invoke.rs:28`) builds a fresh `Vec<Value>` by cloning each argument.

`NativeClosure` returns `Vec<Value>` (`function.rs:33`), which does allocate.
Two options, in order:

1. **Phase 3:** keep it. `recycle::values_of` already pools those vectors, and
   native closures are not the hot path.
2. **Phase 5:** add an opt-in `NativeClosureMulti` writing into
   `&mut [Value]` and returning a count; migrate the iterator closures in
   `stdlib/iter.rs` first, since those run per loop iteration.

`register_all_sigs`, `all_prelude_names`, and `builtin_registries` (`lib.rs:106`)
are untouched — they feed the typechecker and semantic analyser, both of which
run before compilation.

---

## 14. Modules and imports

`crates/saule-interpreter/src/module/` runs the full pipeline per imported module
and caches the result. In the VM this becomes: compile each module to a `Chunk`,
cache the `Chunk`, and execute its `main` proto once. Exported names land in the
importing chunk's module slots.

**This enables a bytecode cache.** A `Chunk` is serializable (the only awkward
members are `Rc`s, which resolve to indices on disk). Caching compiled chunks in
`.saule/cache/` would make `startup` — currently level with Lua, per
`PRODUCTION.md:1100` — faster still on large projects. Out of scope here, but the
design should not preclude it: keep `Chunk` free of anything that cannot be
serialized to indices.

**`saule-lsp` and `saule-db` never execute Saule code.** Their only uses of the
interpreter crate are:

- `saule-lsp`: `module::collect_import_seed`, `module::resolve_import_path`,
  `dynamic_packages::*`, `native_packages::*`, `stdlib::all_prelude_names`, `init`
- `saule-db`: `module::collect_import_seed_io`, `module::SeedIo`

None of that touches evaluation. **The LSP and the docs database are unaffected
by this entire project.** Only `saule-cli` (`run_in`, `call_class_static_method`)
and `saule-wasm` (`run`, `check_and_run`) execute code, and both go through the
five entry points in `lib.rs:128-213`.

---

# Part III — The instruction set

## 15. Complete opcode reference

Semantics below are normative. Where an opcode has a typed variant suffix, the
compiler emits it only when `saule-typeck` proved the operand types; otherwise it
emits the `X` (dynamic) form, which reproduces today's behaviour exactly.

### 15.1 Moves and constants

| Op | Fmt | Semantics |
|---|---|---|
| `MOVE` | ABC | `R[A] := R[B]` |
| `LOADK` | ABx | `R[A] := K[Bx]` |
| `LOADI` | AsBx | `R[A] := Int(sBx)` — small integer literals inline |
| `LOADF` | AsBx | `R[A] := Float(sBx as f64)` — whole-number float literals |
| `LOADBOOL` | ABC | `R[A] := Bool(B != 0)` |
| `LOADNIL` | ABC | `R[A] ..= R[A+B] := Nil` |
| `EXTRAARG` | Ax | Never executed; supplies a 24-bit operand to the preceding instruction |

### 15.2 Upvalues, module slots, statics

| Op | Fmt | Semantics |
|---|---|---|
| `GETUPVAL` | ABC | `R[A] := U[B]` |
| `SETUPVAL` | ABC | `U[B] := R[A]` |
| `CLOSEUP` | ABC | Close all open upvalues pointing at register ≥ `A` |
| `GETMOD` | ABx | `R[A] := M[Bx]` — module-level binding, no hashing |
| `SETMOD` | ABx | `M[Bx] := R[A]` |
| `GETSTAT` | ABC | `R[A] := ClassProto[B].statics[C]` |
| `SETSTAT` | ABC | `ClassProto[B].statics[C] := R[A]` |
| `CLOSURE` | ABx | `R[A] := new Closure(P[Bx])`, binding upvalues per `P[Bx].upvals` |

`GETSTAT`/`SETSTAT` replace the `statics_owner` chain probe (`env.rs:47`,
`env.rs:231`), including the "write targets the *declaring* class" rule
(`value/class.rs:84` — `declaring_static_field`), which the compiler resolves.

### 15.3 Integer arithmetic

`integer` is `i64` and **overflow wraps** (`eval/ops.rs:324`). All of these use
`wrapping_*`.

| Op | Fmt | Semantics |
|---|---|---|
| `ADDI` | ABC | `R[A] := R[B] +ᵂ R[C]` |
| `SUBI` | ABC | `R[A] := R[B] -ᵂ R[C]` |
| `MULI` | ABC | `R[A] := R[B] *ᵂ R[C]` |
| `DIVI` | ABC | `R[A] := R[B] /ᵂ R[C]`; `C == 0` → `DivisionByZero`. Integer division stays integer. |
| `MODI` | ABC | `R[A] := R[B] %ᵂ R[C]`; `C == 0` → `DivisionByZero` |
| `POWI` | ABC | `R[A] := R[B] ^ᵂ R[C]`; negative exponent is an error (`ops.rs:352`) |
| `NEGI` | ABC | `R[A] := -R[B]` |
| `ADDII` | ABC | `R[A] := R[B] +ᵂ sext(C)` — signed 8-bit immediate |
| `SUBII` | ABC | `R[A] := R[B] -ᵂ sext(C)` |
| `MULII` | ABC | `R[A] := R[B] *ᵂ sext(C)` |

The `*II` immediate forms cover `i + 1`, `i - 1`, `n * 2`, `x % 10` — the
overwhelming majority of arithmetic in `loop_arith`, `fib`, and `mandel`. Larger
constants fall back to `LOADK` + `ADDI`.

### 15.4 Float arithmetic

| Op | Fmt | Semantics |
|---|---|---|
| `ADDF` `SUBF` `MULF` `DIVF` `MODF` `POWF` | ABC | `R[A] := R[B] op R[C]`, IEEE 754 |
| `NEGF` | ABC | `R[A] := -R[B]` |

Float division by zero yields infinity, matching `float_op` today.

### 15.5 Bitwise (integer only)

| Op | Fmt | Semantics |
|---|---|---|
| `BAND` `BOR` `BXOR` | ABC | `R[A] := R[B] op R[C]` |
| `SHL` `SHR` | ABC | Saturating shift semantics per `ops.rs:412` — a shift count ≥ 64 yields 0, `SHR` is `SHL` by the negated count |
| `BNOT` | ABC | `R[A] := !R[B]` |

### 15.6 Dynamic arithmetic fallback

| Op | Fmt | Semantics |
|---|---|---|
| `ARITHX` | ABC | `R[A] := binary(op_from_EXTRAARG, R[B], R[C])` — the full `ops::binary` path including `Op*` overload dispatch. Emitted only when an operand's static type is `any`. |
| `UNARYX` | ABC | Ditto for `ops::unary` |

### 15.7 Comparison and branching

Comparisons are **fused with the branch** so an `if` never materializes a `Bool`.

| Op | Fmt | Semantics |
|---|---|---|
| `JMP` | AsBx | `pc += sBx`; if `A > 0`, first `CLOSEUP` at register `A-1` |
| `JLTI` `JLEI` `JGTI` `JGEI` | ABC | If `R[A] op R[B]` (as `i64`), skip the next instruction |
| `JLTF` `JLEF` `JGTF` `JGEF` | ABC | Same, as `f64` |
| `JEQI` `JNEI` | ABC | Integer equality |
| `JEQ` `JNE` | ABC | `values_equal(R[A], R[B])` — the general form, including `Rc::ptr_eq` identity for reference types (`value/mod.rs:151`) |
| `JEQK` | ABC | `R[A] == K[C]` — literal comparison, used by `match` chains |
| `TEST` | ABC | If `R[A].is_truthy() != (C != 0)`, skip the next instruction |
| `TESTSET` | ABC | `and`/`or` value-producing form: if truthiness matches, `R[A] := R[B]` and skip |
| `JNIL` `JNOTNIL` | ABC | Nil test — `?.`, `??`, `!` |

By convention a comparison is always followed by a `JMP`, which the "skip the
next instruction" semantics jumps over. This is Lua's design and it keeps the
comparison opcodes free of a jump operand.

Materializing a boolean where one is genuinely wanted (`local b = x < y`) uses
the comparison plus `LOADBOOL` pair, or the compiler picks a dedicated
`LTI`/`LEI` form producing `R[A] := Bool(...)`. Include those:

| Op | Fmt | Semantics |
|---|---|---|
| `LTI` `LEI` `EQI` | ABC | `R[A] := Bool(R[B] op R[C])`, integer |
| `LTF` `LEF` `EQF` | ABC | Float |
| `EQV` | ABC | `R[A] := Bool(values_equal(R[B], R[C]))` |
| `NOT` | ABC | `R[A] := Bool(!R[B].is_truthy())` |

### 15.8 Loops

| Op | Fmt | Semantics |
|---|---|---|
| `FORPREP_I` | AsBx | Validate `R[A]` (counter), `R[A+1]` (limit), `R[A+2]` (step) as integers; error on zero step; jump `sBx` if the loop body will not run; else initialize `R[A+3]` (user variable) |
| `FORLOOP_I` | AsBx | `R[A] +=ᵂ R[A+2]`; on overflow, exit; if still in range, set `R[A+3]` and jump back `sBx` |
| `FORPREP_F` / `FORLOOP_F` | AsBx | Float variants |
| `ITERPREP` | ABx | Resolve the iteration source in `R[A]`: snapshot a table (array, then sorted map entries) or resolve a closure driver (calling `iter()` on an instance). Store control state in `R[A]..R[A+2]`. Jump `Bx` if empty. |
| `ITERNEXT` | AsBx | Advance; write key/value into `R[A+3]`, `R[A+4]`; jump back `sBx` while values remain |

### 15.9 Tables

| Op | Fmt | Semantics |
|---|---|---|
| `NEWT` | ABC | `R[A] := new table`, array capacity hint `B`, map capacity hint `C` |
| `SETLIST` | ABC | Append `R[A+1]..R[A+B]` to `R[A]`'s array part in bulk |
| `GETARR` | ABC | `R[A] := R[B].array[R[C] - 1]`; bounds-checked, 1-based |
| `SETARR` | ABC | `R[A].array[R[B] - 1] := R[C]` |
| `GETMAP` | ABC | `R[A] := R[B].map[key(R[C])]` |
| `SETMAP` | ABC | `R[A].map[key(R[B])] := R[C]` |
| `GETMAPK` | ABC | `R[A] := R[B].map[K[C]]`, with the constant's hash precomputed |
| `SETMAPK` | ABC | `R[A].map[K[B]] := R[C]` |
| `GETIDX` | ABC | Fully dynamic `read_index` — receiver type unknown |
| `SETIDX` | ABC | Fully dynamic index write |
| `APPEND` | ABC | Push `R[B]` onto `R[A]`'s array part |
| `LEN` | ABC | `R[A] := #R[B]` — array length, string char count, or `OpLen` dispatch |

### 15.10 Classes and instances

| Op | Fmt | Semantics |
|---|---|---|
| `NEW` | ABx | `R[A] := new instance of ClassProto[Bx]`, fields initialized from the template or via `field_init` |
| `GETF` | ABC | `R[A] := R[B].fields[C]` — static slot |
| `SETF` | ABC | `R[A].fields[B] := R[C]` — static slot |
| `GETFX` | ABC | `R[A] := R[B].<K[C]>` via inline cache; falls back to `read_member` |
| `SETFX` | ABC | Dynamic field write via inline cache |
| `CALLM` | ABC | `R[A]` = receiver, args in `R[A+1]..R[A+B-1]`, vtable slot `C`, **one** result into `R[A]` |
| `CALLM_MR` | ABC | Multi-return method call; `B` = nargs+1, `C` = nret+1, vtable slot in `EXTRAARG` |
| `CALLIF` | ABC | Interface dispatch; `C` = interface method slot, interface index in `EXTRAARG` |
| `CALLSTAT` | ABC | Static method call; `B` = nargs+1, class and slot in `EXTRAARG` |
| `SUPER` | ABC | `self.super(args)` — dispatch to the parent's `init`. Replaces the `SUPER_OWNER_BINDING` scope hack at `eval/expr/mod.rs:38`. |
| `ISA` | ABC | `R[A] := Bool(R[B] is an instance of ClassProto[C] or a subclass)` |

### 15.11 Enums and `match`

| Op | Fmt | Semantics |
|---|---|---|
| `GETTAG` | ABC | `R[A] := Int(tag of the enum variant in R[B])` |
| `SWITCH` | ABx | Jump through jump table `Bx`, indexed by `R[A]`; out-of-range falls through |
| `JIFTAG` | ABC | If `R[A]`'s tag == `B`, skip the next instruction |
| `VARIANT` | ABx | `R[A] := EnumProto[Bx].by_tag[C]` — singleton variant reference |
| `NEWVAR` | ABC | Construct a tuple variant: payload from `R[A+1]..R[A+B-1]` |
| `UNWRAP` | ABC | `R[A] := payload of the variant in R[B]` |

### 15.12 Nullability

| Op | Fmt | Semantics |
|---|---|---|
| `JNIL` / `JNOTNIL` | ABC | (listed in §15.7) — the branch behind `?.`, `??`, and nil-narrowing |
| `COALESCE` | ABC | `R[A] := if R[B] is nil { R[C] } else { R[B] }` — non-short-circuiting form for constant RHS; the general `??` uses `JNOTNIL` + `JMP` to preserve laziness |
| `UNWRAPNIL` | ABC | `x!` — `R[A] := R[B]`, or `ForceUnwrapNil` error |
| `CASTCHK` | ABC | `x as T` — `R[A] := R[B]` if the runtime type test passes, else `Nil`. Type descriptor in `K[C]`; never throws (per `eval/expr/mod.rs:97`). |

### 15.13 Calls and returns

| Op | Fmt | Semantics |
|---|---|---|
| `CALL` | ABC | `R[A]` = callee, args `R[A+1]..R[A+B-1]` (`B=0`: to `top`), results into `R[A]..R[A+C-2]` (`C=0`: all, set `top`) |
| `CALLK` | ABC | Statically resolved callee: proto index in `EXTRAARG`, args `R[A]..R[A+B-2]`. Skips the callee load, the callability test, and the arity check. |
| `CALLNAT` | ABC | Native call: `NativeFn` index in `EXTRAARG`, args passed as `&stack[base+A .. base+A+B-1]` |
| `TAILCALL` | ABC | Reuse the current frame; args move down to `base`. Enables unbounded tail recursion. |
| `RET` | ABC | Return `R[A]..R[A+B-2]`; `B=0` returns to `top` |
| `RET0` | ABC | Return no values |
| `RET1` | ABC | Return `R[A]` — by far the most common shape |

### 15.14 Strings

| Op | Fmt | Semantics |
|---|---|---|
| `CONCAT` | ABC | `R[A] := R[B] .. R[B+1] .. … .. R[C]` — n-ary, **one** allocation sized from the operands |
| `TOSTR` | ABC | `R[A] := display_value(R[B])`, dispatching `OpToString` when the operand is an instance |

`a .. b .. c` today allocates twice (`eval/ops.rs:153`); n-ary `CONCAT` allocates
once with `String::with_capacity` computed from all operands.

### 15.15 Errors

| Op | Fmt | Semantics |
|---|---|---|
| `THROW` | ABC | Set the pending value from `R[A]` and unwind to the nearest matching handler (§12.1) |
| `CHKTY` | ABC | Runtime type test used by `catch`; `R[A] := Bool(R[B] matches K[C])` |

---

## 16. Superinstructions and specialization

Phase 5 work, listed here so the encoding leaves room. Each must be justified by
a profile before it is added; every one of them is a maintenance cost.

Candidates, in expected-value order:

1. **`GETF_CALLM`** — `obj.field.method(...)`, extremely common in OO code.
2. **`FORLOOP_GETARR`** — the `for i = 1 to #t do … t[i] … end` shape that
   dominates `array` and `sort`.
3. **`ADDII_MOVE`** — accumulator updates.
4. **`GETUPVAL_CALL`** — calling a captured function, the `closure` benchmark's
   inner loop.
5. **`JLTI_ADDII`** — the loop-condition-plus-increment pair.

A superinstruction is only worth it when the pair appears in a hot loop *and* the
fused version removes a register round-trip. Measure with a per-opcode-pair
histogram collected under a `--profile-bytecode` flag.

**The collector exists** — `crates/saule-vm/src/profile.rs`, wired into the
dispatch loop and printed by `saule run --profile-bytecode`:

```bash
cargo build --release --features profile -p saule-cli
./target/release/saule run --profile-bytecode benchmarks/sau/fib.sau
```

It is behind a cargo feature because the counting copy of the loop costs 2–3%
on the call-heavy benchmarks by *existing*, with every counter compiled out;
`VM_TASKS.md`'s Phase 5 records the measurement. A pair is counted only when
the two instructions were **statically adjacent** — neighbouring words of one
proto — because that is the only adjacency the emitter can fuse. A back-edge
into a loop body is not one.

Its first readings argue for the emission peephole (§17) ahead of any of the
five candidates above: `MOVE` is 30% of `fib`, `LOADI` and `MOVE` are half of
`loop_arith`, and `fib` runs `LTI TEST` a million times as a pair while
`JLTI` — the fused form, already in the instruction set — goes unemitted.

---

# Part IV — The compiler

## 17. Compiler passes

Input: a `saule_ast::Module` that has already passed `saule_semantic::analyze`
and `saule_typeck::check`. Output: a `Chunk`.

```
Module (AST)
  │
  ├─ [existing] semantic::analyze  ──► registries + ResolveTable (new, §21.1.5)
  ├─ [existing] typeck::check      ──► TypeTable (new, §21.1.4)
  │
  ├─ Pass 1: layout
  │     Build ClassProto layouts, vtables, itables; EnumProto tags;
  │     module slot assignment. Order classes by inheritance depth.
  │
  ├─ Pass 2: codegen
  │     Recursive walk emitting instructions. Register allocation is a
  │     stack discipline (§18). Consults TypeTable for opcode selection
  │     and ResolveTable for every name.
  │
  ├─ Pass 3: patch
  │     Resolve forward jump labels, build match jump tables,
  │     finalize the line table.
  │
  └─ Pass 4: verify (debug builds only)
        Every register index < max_regs; every jump lands in range and on
        an instruction boundary; every constant/proto/class index valid;
        stack depth consistent at every join point.
```

Pass 4 is what licenses `get_unchecked` in the dispatch loop. Run it under
`debug_assertions` and in the test suite; skip it in release for trusted chunks,
run it always for deserialized ones if a bytecode cache lands later.

### 17.1 Where the compiler lives

New crate `saule-vm`, depending on `saule-interpreter` for `Value`,
`TableObject`, `RuntimeError`, and the stdlib. **Not** the other way round — the
interpreter crate must not gain a dependency on the VM, so the tree-walker stays
buildable in isolation.

```
crates/saule-vm/
├── Cargo.toml
└── src/
    ├── lib.rs            — run / run_chunk / run_program / disassemble
    ├── op.rs             — the opcode table, operand layouts, Instruction
    ├── disasm.rs         — `saule disasm <file>`, essential for debugging
    ├── program.rs        — resolving an import graph into a set of chunks
    ├── profile.rs        — opt-in bytecode profiling (§16)
    ├── chunk/
    │   ├── mod.rs        — Chunk: one compiled module and its pools
    │   ├── proto.rs      — Proto, UpvalDesc, Handler, LineEntry, InlineCache
    │   └── desc.rs       — ClassProto, EnumProto, TypeDesc, StaticSlot, ...
    ├── compile/
    │   ├── mod.rs        — the four-pass driver
    │   ├── layout.rs     — Pass 1
    │   ├── class.rs      — compiling a class body against its layout
    │   ├── match_.rs     — pattern compilation and jump tables
    │   ├── verify.rs     — Pass 4
    │   ├── ctx/          — the compiler's own state
    │   │   ├── mod.rs    — the Compiler struct, construction, finish
    │   │   ├── func.rs   — one function: scopes, locals, upvalue capture
    │   │   ├── emit.rs   — instructions, jumps, labels, patches, constants
    │   │   ├── regalloc.rs — §18
    │   │   ├── operand.rs  — purity and in-place reads
    │   │   ├── resolve.rs  — a name → a slot, a static, a callee
    │   │   └── coerce.rs   — §19 argument binding, declared-type coercion
    │   ├── expr/         — expression codegen
    │   │   ├── mod.rs    — expr_to / expr_tmp / expr_results
    │   │   ├── ident.rs  arith.rs  call.rs  args.rs  results.rs
    │   │   └── pipe.rs   member.rs  safe.rs  literal.rs
    │   └── stmt/         — statement codegen
    │       ├── mod.rs    — block / stmt dispatch
    │       └── decl.rs  assign.rs  control.rs  loops.rs  ret.rs  try_catch.rs
    └── vm/
        ├── mod.rs        — Vm, VmShared, and the ways in
        ├── dispatch.rs   — the interpreter loop, deliberately one function
        ├── call.rs       — frames, tail calls, natives, vtable lookup
        ├── unwind.rs     — finding a handler, and the type tests it applies
        ├── build.rs      — chunk protos → runtime class and enum objects
        ├── frame.rs  upval.rs
        └── ops.rs        — reading operands out of registers, numeric helpers
```

**The dispatch loop is one function on purpose.** `vm/dispatch.rs` holds
`execute_loop` whole: its arms borrow loop-local state across the entire body,
and it is monomorphised twice over `PROFILE` — the second copy alone measured
2-3% on the call-heavy benchmarks through code layout, with no profiling
instruction executing. Splitting arms out of it is a performance change, not a
tidying one.

Write `disasm.rs` **first**. Debugging a bytecode compiler without a disassembler
is miserable.

---

## 18. Register allocation

A stack discipline, not graph colouring. This is what every Lua-family compiler
does and it is sufficient.

- `free_reg` marks the first unused register in the frame.
- Locals occupy registers `0..n_locals` in declaration order and stay put for
  their whole lexical extent.
- Temporaries allocate at `free_reg` and are released in LIFO order as
  subexpressions complete.
- `max_regs` is the high-water mark, recorded on the proto.

For a call, the compiler bumps `free_reg` to a fresh window, emits the callee
into the first slot and each argument into the next — so the arguments are
**constructed in place** in the callee's future frame (§6.2). No copying.

Block scoping: on leaving a block, `free_reg` resets to the block's entry value.
If the block declared anything a closure captured, emit `CLOSEUP` at that
register first.

Peephole opportunities worth taking during emission:

- `MOVE r, r` → drop.
- `LOADK` immediately consumed by an arithmetic op with a small integer value →
  fold into the `*II` immediate form.
- Comparison followed by `LOADBOOL`/`TEST` → fuse into the branch form.
- A jump to the next instruction → drop.

---

## 19. Compile-time argument binding

This section is short but it is the single biggest call-path win, so it deserves
its own heading.

**Everything `bind_params` does at runtime is decided at compile time.**

Given a call site's `Vec<CallArg>` and the callee's `Vec<Param>`, the compiler:

1. Calls the existing `saule_ast::resolve_arg_slots` (`expr.rs:327`) — the same
   function the typechecker already uses — to get `Vec<Option<usize>>`: which
   parameter slot each argument fills, including the trailing-block rule
   (`trailing_block_param`, `expr.rs:285`).
2. Reports arity, duplicate, unknown-name, and positional-after-named errors
   **at compile time**. These are already typecheck errors; the compiler asserts
   rather than re-diagnoses.
3. Emits argument evaluation **in parameter order**, directly into the callee's
   register window. Named arguments are reordered by the compiler; the runtime
   never sees a name.
4. Inlines defaults. A parameter with no argument gets its `Param.default`
   expression compiled inline at the call site (matching today's semantics, where
   `bind_params` evaluates the default in the *callee's* scope — note the
   compiler must respect that scoping, so a default referencing another parameter
   compiles against the callee's frame; in practice defaults are constants and
   fold to a `LOADK`).
5. Fills a nullable parameter with `LOADNIL` (`binding.rs:207`).
6. For a variadic parameter, emits `NEWT` + `SETLIST` over the surplus
   registers.

At runtime, `CALLK` does: push a frame, jump. **Zero** scanning, zero
`Vec<Option<Value>>`, zero hash inserts.

The default-scoping subtlety in (4) is the one genuine correctness trap in this
section. The safe implementation: compile each defaulted parameter's initializer
into the *callee's* prologue, guarded by a `JNOTNIL` on a sentinel — or, simpler
and preferred, compile a per-arity **entry stub** for callees with defaults, so
`f(a)` and `f(a, b)` jump to different entry points in the same proto. The stub
approach costs nothing at runtime and keeps default evaluation in the callee's
scope where it belongs.

---

# Part V — Getting there

## 20. Expected performance

Per-phase, against the `PRODUCTION.md` Appendix A baseline:

| Change | Mechanism | Expected effect |
|---|---|---|
| Register frames, slot locals, no `Environment` | Deletes §1.1 and §1.2 entirely | **2–3× overall** |
| Compile-time argument binding + `CALLK` | Deletes §1.3 | **1.5–2× on call-heavy** (`fib`, `closure`, `oop`) |
| Typed arithmetic + `*II` immediates | Deletes §1.4's operator half | **1.5–2× on** `loop_arith`, `mandel` |
| Slot fields + vtables + `NEW` template | Deletes §1.4's member half and §1.5 | **2–4× on** `oop`; large memory drop |
| `match` jump tables | O(1) instead of O(arms) | Large on match-heavy code; not in the current benchmark set |
| n-ary `CONCAT` | One allocation instead of n−1 | **~1.3× on** `strings` |
| Inline caches, superinstructions | Phase 5 | Last 20–30% |

**Projected landing zone: roughly parity with PUC Lua overall**, ahead of it on
`oop` and integer arithmetic (where Saule's static types remove work Lua must do
dynamically), behind on nothing. That is 5–11× → ~1×.

LuaJIT will remain 5–20× ahead. That gap is a tracing JIT and it is explicitly
not a goal.

Two caveats worth stating plainly:

- These are estimates from the structure of the costs, not measurements. The
  Phase 2 exit criterion (§21.3) is a real number on `loop_arith` and `fib`; if
  it comes in under 2×, the design assumptions need revisiting before Phase 3.
- `map` (1.2× Lua) and `startup` (1.2×) will barely move. `map` is dominated by
  hashing inside `TableObject`, which the VM does not change; `startup` is
  front-end cost, and compilation will make it marginally *worse* until a
  bytecode cache exists. Watch `startup` for regression — being level with Lua
  there is called out in `PRODUCTION.md:1100` as a real achievement and should
  not be traded away.

---

## 21. Implementation plan

The ordering constraint is absolute: **`./run_tests.sh` passes at every commit,
and the tree-walker remains the default until Phase 4.**

### 21.1 Phase 0 — Foundations (2–4 weeks)

Every item here is a standalone improvement to the existing interpreter. None of
them depends on the VM existing. If the VM project is cancelled after Phase 0,
all of this work still pays for itself.

---

#### 0.1 — `NodeId` on `Spanned`

*Files:* `crates/saule-ast/src/lib.rs`, plus 13 struct-literal sites across
`saule-parser`, `saule-lexer`, `saule-fmt`, `saule-cli`.

Side tables keyed by AST node need a stable key. Add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(pub u32);
impl NodeId { pub const NONE: NodeId = NodeId(u32::MAX); }

#[derive(Debug, Clone)]
pub struct Spanned<T> {
    pub value: T,
    pub span: Range<usize>,
    pub id: NodeId,
}
```

Two details that matter:

- **`PartialEq` must be implemented manually and ignore `id`.** `Spanned` derives
  `PartialEq` today and parser tests compare parsed trees against hand-built
  ones. Deriving over the new field would break every one of them.
- **The parser does not assign ids.** `Spanned::new` sets `NodeId::NONE`; a new
  `saule_ast::assign_ids(&mut Module)` numbers nodes in a deterministic pre-order
  walk. The pipeline calls it once after parsing. This keeps all 73
  `Spanned::new` call sites unchanged; only the 13 struct-literal sites need a
  `id: NodeId::NONE` field.

*Independent value:* the LSP can key caches on node identity instead of spans.

*Risk:* low. Mechanical.

---

#### 0.2 — Slot-based instance fields

*Files:* `crates/saule-interpreter/src/value/class.rs`,
`eval/expr/members.rs`, `eval/expr/construct.rs`, `eval/stmt/assign.rs`,
`eval/stmt/classes.rs`, `value/mod.rs` (the `to_display_string` arm).

Replace `InstanceObject.fields: HashMap<String, Value>` with `Vec<Value>` plus a
shared `Rc<FieldLayout>` on `ClassObject` (§4.4). Build the layout in
`eval/stmt/classes.rs` when the class value is constructed, parent fields first.

`read_member` (`members.rs:50`) becomes `layout.index.get(name)` → indexed load:
the same single hash probe as today, but against a **per-class** map instead of a
**per-instance** one.

*Independent value:* large. Every instance stops carrying its own hash table and
its own cloned `String` keys. Expect a substantial drop in the memory figures in
`PRODUCTION.md` §"Memory behaviour" and a measurable gain on `oop`.

*Risk:* medium — it touches instance construction, field assignment, and display.
`tests/` has good class coverage (`field_assign_types.sau`, `inheritance*.sau`,
`private_fields.sau`, and others).

---

#### 0.3 — Flattened method tables

*Files:* `crates/saule-interpreter/src/value/class.rs`,
`eval/stmt/classes.rs`.

`lookup_method` (`class.rs:52`) walks the parent chain. Flatten instead: build
each class's `methods` map as a copy of its parent's with overrides applied, at
class-construction time. Lookup becomes one probe regardless of depth.

Add `vindex: HashMap<Rc<str>, u16>` and `vtable: Vec<Rc<FunctionObject>>`
alongside — unused by the tree-walker, consumed by the VM later.

*Independent value:* removes a hash probe per inheritance level per method call.

*Risk:* low. Watch memory on deep hierarchies (each class now stores the full
method set); in practice hierarchies are shallow.

---

#### 0.4 — Dense enum tags

*Files:* `crates/saule-interpreter/src/value/enum_.rs`,
`eval/stmt/enums.rs`, `eval/expr/match_.rs`.

Add `tag: u32` to `EnumVariantObject` and `by_tag: Vec<Rc<EnumVariantObject>>` to
`EnumObject` (§4.4). Change `match_.rs`'s variant test from comparing
`enum_name`/`variant_name` strings to comparing tags.

*Independent value:* `match` over enums stops doing string comparisons.

*Risk:* low.

---

#### 0.5 — Typeck publishes a type table

*Files:* `crates/saule-typeck/src/lib.rs`, `expr/infer.rs`, `expr.rs`, `stmt.rs`.

```rust
pub type TypeTable = HashMap<NodeId, Type>;

pub fn check_with_types(module: &Module) -> (Vec<TypeCheckError>, TypeTable);

pub fn check(module: &Module) -> Vec<TypeCheckError> {
    check_with_types(module).0          // existing signature unchanged
}
```

The work is threading a `&mut TypeTable` through the walker and recording the
result of `infer` (`expr/infer.rs:14`) at each node. `check_stmt` and
`check_expr` are plain recursive walks with no context parameter today — the
crate already leans on thread-locals for `CURRENT_CLASS`, `RETURN_TY`, and
friends (`state.rs:38-75`), so a thread-local `TypeTable` sink is consistent with
the existing style and avoids touching every signature.

**Coverage matters more than completeness.** `infer` returns `Option<Type>` and
is documented as deliberately partial (`lib.rs:17`). Every `None` is an opcode
that degrades to its dynamic form. Add a `--dump-type-coverage` debug flag that
reports the fraction of expression nodes with a recorded type, and treat raising
it as ongoing work.

*Independent value:* the LSP gets inlay hints and precise hover types for free.

*Risk:* medium — this is the largest Phase 0 item and the one most likely to
uncover places where inference is weaker than assumed.

---

#### 0.6 — Semantic publishes a binding table

*Files:* `crates/saule-semantic/src/resolve.rs`, `resolve/scope.rs`,
`resolve/exprs.rs`, `resolve/decls.rs`, `lib.rs`.

The `Resolver` (`resolve.rs:73`) already walks with a `Vec<HashSet<String>>`
scope stack and already knows, for every identifier, whether it resolves to a
local, a module-level declaration, an import, or the prelude — that is precisely
what `SemanticError::UndefinedName` is checking. It just discards the answer.

```rust
pub enum Binding {
    Local  { frame_depth: u16, slot: u16 },
    Upvalue{ index: u16 },
    Module { slot: u16 },
    Static { class: Rc<str>, slot: u16 },
    ClassStatic,                 // resolved through statics_owner
    Prelude{ name: Rc<str> },
    SelfRef,
}
pub type ResolveTable = HashMap<NodeId, Binding>;

pub fn analyze_with_bindings(module: &Module, seed: ModuleSeed)
    -> (Vec<SemanticError>, ResolveTable);
```

Two substantive upgrades to the walker:

- **Slot assignment.** Frames become ordered `Vec<Rc<str>>` rather than
  `HashSet<String>`, so each binding gets an index.
- **Real free-variable analysis.** Determine, per lambda, exactly which
  enclosing bindings it references, and classify each as a parent register or a
  parent upvalue. This *replaces* `crates/saule-interpreter/src/capture.rs` and
  its documented bail-out (`capture.rs:34-47`).

*Independent value:* the tree-walker can adopt the precise capture set
immediately, which removes `capture.rs`'s over-approximation and its whole-scope
fallback — a real leak fix, closing part of the issue `PRODUCTION.md:218`
describes.

*Risk:* medium-high. This is the subtlest Phase 0 item. Closure semantics are
covered by `tests/closure_capture.sau`, `tests/anon_func.sau`, and the memory
fixtures in `PRODUCTION.md` §3.2 — run those explicitly.

---

#### Phase 0 exit criteria

- `./run_tests.sh` green.
- `REPS=3 python3 benchmarks/bench.py` shows no regression, and ideally a
  measurable gain on `oop`.
- Peak-memory figures for the fixtures in `PRODUCTION.md` §3.2 improve or hold.
- Type coverage reported and recorded as a baseline.

---

### 21.2 Phase 1 — Crate skeleton (1 week)

*New:* `crates/saule-vm/`.

- `chunk.rs`, `op.rs` with the full opcode enum from §15, encode/decode helpers,
  and round-trip property tests.
- `disasm.rs` producing readable output. Wire `saule disasm <file>` into the CLI.
- The dispatch loop skeleton handling `MOVE`, `LOADK`, `LOADI`, `RET1`, `JMP` —
  enough to execute a hand-written chunk.
- No compiler yet. No integration with the pipeline.

*Exit:* a hand-assembled chunk computing `1 + 2` runs and returns `3`; the
disassembler prints it legibly.

---

### 21.3 Phase 2 — Core VM (4–6 weeks)

Target: run `benchmarks/sau/loop_arith.sau`, `fib.sau`, and `array.sau`.

Language subset: top-level `fn`, `local`, assignment, integer and float
arithmetic, comparison, `if`/`while`/`repeat`, numeric `for`, `break`/`continue`,
`return` (single and multi), calls to Saule functions and natives, table literals
and indexing, lambdas and closures.

Not yet: classes, enums, `match`, `try`/`catch`, `for … in`, imports, pipes.

Integration: a new entry point `saule_vm::run_in(module, ...)`, selected by
`SAULE_ENGINE=vm` or a `--vm` CLI flag. **The tree-walker remains the default.**
Anything the compiler cannot yet handle returns
`CompileError::Unsupported(node)`, and the CLI falls back to the tree-walker with
a warning — so `--vm` is usable long before it is complete.

*Exit criteria:*
- The three benchmarks produce byte-identical output under both engines.
- `loop_arith` and `fib` are at least **2.5× faster** than the tree-walker. If
  not, stop and diagnose before proceeding — the whole design rests on this.

---

### 21.4 Phase 3 — Full language (4–6 weeks)

In dependency order:

1. **Classes** — `ClassProto`, layouts, vtables, `NEW`, `GETF`/`SETF`,
   `CALLM`, `CALLSTAT`, `SUPER`, statics. Unlocks `oop.sau`.
2. **Interfaces** — itables, `CALLIF`, inline caches.
3. **Enums and `match`** — tags, `SWITCH`, jump tables, pattern compilation,
   guards, exhaustiveness (already checked by `saule-typeck`, so the compiler
   may assume it).
4. **`try`/`catch`/`throw`** — handler tables, unwinding, `CHKTY`.
5. **`for … in`** — both the table-snapshot and closure-driver paths (§11.2).
6. **Operator overloading** — compile-time contract resolution (§8.7).
7. **Nullability** — `?.`, `??`, `!`, `as`.
8. **Pipes** — `Expr::Pipe` (`expr.rs:100`) is a straightforward lowering to
   chained `CALLK`s with the upstream value in register 0 of each argument
   window.
9. **Imports and modules** — per-module chunks, module slots, cross-module class
   layouts from the `ModuleSeed` registry.
10. **Variadics, trailing blocks, named arguments, defaults** — §19.

*Exit criteria:*
- All 91 `tests/*.sau` and all `tests/ui/*.sau` behave identically under both
  engines.
- All 10 benchmarks run under `--vm`.
- The differential harness (§23.2) is green across `examples/` and `www/`.

---

### 21.5 Phase 4 — Flip the default (1–2 weeks)

- `--vm` becomes the default; `--interp` selects the tree-walker.
- `saule-wasm` switches `run` / `check_and_run` to the VM.
- One release ships with both engines and a documented escape hatch.
- Update `PRODUCTION.md` §"How fast is it?", the grade table at line 445, and
  Appendix A with real numbers.
- `saule-lsp` and `saule-db` need **no changes** (§14).

Keep the tree-walker in-tree for at least one full release cycle. It is the
differential oracle, and it is a much simpler thing to debug against when a VM
bug is suspected.

---

### 21.6 Phase 5 — Optimization (ongoing)

Only with a profile in hand:

1. Inline caches for `GETFX` / `CALLIF`.
2. Superinstructions from a measured opcode-pair histogram (§16).
3. `NativeClosureMulti` for `stdlib/iter.rs`.
4. Constant-key hash precomputation.
5. Dispatch-loop threading experiments.
6. Bytecode caching for `startup` on large projects.
7. **Only then**: reconsider NaN-boxing, with numbers.

---

## 22. Keeping the tree-walker alive

The requirement that the interpreter keeps working is a design constraint, not an
afterthought. Four things enforce it:

**1. Dependency direction.** `saule-vm` depends on `saule-interpreter`, never the
reverse. The interpreter crate compiles and passes its tests with `saule-vm`
deleted from the workspace.

**2. Additive-only signatures.** Every Phase 0 API change adds a function and
reimplements the old one in terms of it:

```rust
pub fn check(module: &Module) -> Vec<TypeCheckError> {
    check_with_types(module).0
}
pub fn analyze_with_seed(module: &Module, seed: ModuleSeed) -> Vec<SemanticError> {
    analyze_with_bindings(module, seed).0
}
```

No caller in `saule-cli`, `saule-lsp`, `saule-wasm`, or `saule-db` changes.

**3. Shared runtime types.** `Value`, `TableObject`, `ClassObject`,
`EnumObject`, `RuntimeError`, and the whole stdlib stay in `saule-interpreter`
and are used by both engines. The Phase 0 changes to `InstanceObject`,
`ClassObject`, and `EnumVariantObject` are made **in the tree-walker first**, so
they are exercised by the full test suite for weeks before the VM reads them.

**4. Engine selection at the entry points.** The five entry points in
`crates/saule-interpreter/src/lib.rs:128-213` are the entire execution surface.
The CLI dispatches on a flag; everything upstream of them is shared.

The failure mode to guard against is Phase 0's refactors being justified *only*
by the VM. Each item in §21.1 is written to stand alone precisely so a stalled VM
project does not leave the interpreter worse off.

---

## 23. Testing strategy

### 23.1 What exists

- `tests/*.sau` — 91 fixtures that must run and exit 0.
- `tests/ui/*.sau` — fixtures that must fail, each pinning a specific diagnostic.
- `run_tests.sh` — gates both, honours `SAULE_BIN`.
- `benchmarks/bench.py` — supports `new=` / `old=` comparison.

This is a good safety net and it is the reason this project is tractable.

### 23.2 What to add

**A differential harness.** A `saule check-engines <file>` mode that runs a
program under both engines and compares stdout, stderr, and exit code. Wire it
into `run_tests.sh` behind a flag so CI runs the whole fixture set both ways.
This is the single highest-value piece of new test infrastructure — it turns
every existing fixture into a VM conformance test at no authoring cost.

**Bytecode round-trip tests.** Encode/decode property tests over the instruction
formats.

**Verifier tests.** Hand-built malformed chunks must be rejected by Pass 4, not
crash the VM.

**Closure semantics fixtures.** Per-iteration capture, self-recursive locals, and
upvalue closing are the areas where a subtle divergence is most likely and least
likely to be caught by existing tests. Add fixtures that assert *values*, not
just successful exit.

**Memory fixtures.** The variants in `PRODUCTION.md` §3.2 should become
executable fixtures with recorded peak-RSS bounds, so Phase 0.2 and 0.6 can be
verified rather than assumed.

**Benchmark regression gate.** `PRODUCTION.md:604` already proposes this. It
becomes load-bearing here: every phase must show its expected gain and no
regression elsewhere, with the ~3% noise floor from `PRODUCTION.md:1135` in mind.

---

## 24. Risks and open questions

### 24.1 Type coverage is lower than assumed

**Risk:** `infer` (`expr/infer.rs:14`) returns `None` more often than expected —
the crate's own docs call the inference "intentionally partial" (`lib.rs:17`) —
and most arithmetic degrades to `ARITHX`, collapsing the arithmetic win.

**Mitigation:** Phase 0.5 ships `--dump-type-coverage` before any VM work
depends on it. Measure on `benchmarks/sau/`, `examples/`, and `tests/` first. If
coverage on arithmetic operands is below ~90%, budget time to strengthen
inference before Phase 2, or accept the reduced projection.

### 24.2 Cross-module class layout divergence

**Risk:** The compiler computes a class's field layout from the
`saule-semantic` registry; the runtime `ClassObject` computes its own. If they
disagree — because of import ordering, a seeded vs. locally-declared class, or
the `builtins` snapshot precedence rules in `lib.rs:113-120` — `GETF` reads the
wrong field. Silently.

**Mitigation:** make the layout **single-sourced**: the compiler produces the
`ClassProto`, and the runtime `ClassObject` is built *from* it, not
independently. Additionally, in debug builds, have `GETF` assert the receiver's
class layout pointer matches the one the instruction was compiled against.

### 24.3 Reference cycles remain

The VM does not fix the `Rc` cycle problem `PRODUCTION.md:218` describes. A
closure that closes over itself still leaks (§7.3).

What the VM *does* change is that it makes a future fix tractable: the GC roots
become the register stack, module slots, and open upvalues — a small, explicit,
enumerable set — rather than an arbitrary graph of `Rc<RefCell<Environment>>`.
A cycle collector or a tracing GC becomes a well-defined project instead of an
open-ended one. Worth stating as a strategic benefit; not a deliverable here.

### 24.4 256 registers

A machine-generated or unusually large function body could exceed the frame
limit. Must be a clean `CompileError`, never a panic, and the message must say
what to do (split the function). Consider a `--max-regs` diagnostic mode that
reports the high-water mark per function so the limit is discoverable.

### 24.5 `startup` regression

Compilation is work the tree-walker does not do. `startup` is currently level
with Lua, which `PRODUCTION.md:1100` calls out as a real achievement.

**Mitigation:** keep the compiler single-pass over the AST with no intermediate
IR; measure `startup` every phase; treat any regression beyond noise as a
blocker. A bytecode cache (§14) is the long-term answer if it becomes a problem.

### 24.6 Effort

Realistic single-developer estimate:

| Phase | Estimate |
|---|---|
| 0 — Foundations | 2–4 weeks |
| 1 — Skeleton | 1 week |
| 2 — Core VM | 4–6 weeks |
| 3 — Full language | 4–6 weeks |
| 4 — Flip default | 1–2 weeks |
| **Total to parity** | **~3–4 months** |
| 5 — Optimization | ongoing |

Phase 0 is the one people skip. Retrofitting type information into a finished
compiler means rewriting codegen, so the 2–4 weeks are not optional.

### 24.7 Open questions

1. **Should `Value` and the stdlib move to a `saule-runtime` crate?** Cleaner
   long-term; a large mechanical move. Recommendation: no, not now. Have
   `saule-vm` depend on `saule-interpreter`. Revisit if the tree-walker is
   eventually deleted.
2. **Should the tree-walker be deleted after Phase 4?** Recommendation: no. It is
   the differential oracle and it is ~13k lines that already work.
3. **Do defaults compile to entry stubs or to guarded prologues?** §19 recommends
   stubs. Decide with a microbenchmark in Phase 3.
4. **Does `SWITCH` want a dense jump table or a binary search?** Dense for enums
   (tags are dense by construction); binary search for sparse integer matches.
   Compiler picks based on density.

---

## Appendix A — file-by-file change map

### Modified in Phase 0 (tree-walker keeps working, and gets faster)

| File | Change | Phase |
|---|---|---|
| `crates/saule-ast/src/lib.rs` | `NodeId`, `Spanned.id`, manual `PartialEq`, `assign_ids` | 0.1 |
| `crates/saule-parser/src/expr/matching.rs` | one `Spanned` literal gains `id` | 0.1 |
| `crates/saule-lexer/src/lib.rs` | one `Spanned` literal gains `id` | 0.1 |
| `crates/saule-fmt/tests/corpus.rs`, `crates/saule-cli/src/fmt.rs` | `Spanned` literals gain `id` | 0.1 |
| `crates/saule-interpreter/src/value/class.rs` | `FieldLayout`, `InstanceObject.fields: Vec<Value>`, flattened methods, `vtable`/`vindex` | 0.2, 0.3 |
| `crates/saule-interpreter/src/eval/expr/members.rs` | `read_member` goes through the layout | 0.2 |
| `crates/saule-interpreter/src/eval/expr/construct.rs` | `init_fields` fills slots, not a map | 0.2 |
| `crates/saule-interpreter/src/eval/stmt/assign.rs` | field writes go through the layout | 0.2 |
| `crates/saule-interpreter/src/eval/stmt/classes.rs` | build layout + flattened method table at class construction | 0.2, 0.3 |
| `crates/saule-interpreter/src/value/mod.rs` | `to_display_string` instance arm | 0.2 |
| `crates/saule-interpreter/src/value/enum_.rs` | `tag`, `by_tag` | 0.4 |
| `crates/saule-interpreter/src/eval/stmt/enums.rs` | assign tags at declaration | 0.4 |
| `crates/saule-interpreter/src/eval/expr/match_.rs` | compare tags, not names | 0.4 |
| `crates/saule-typeck/src/lib.rs` | `check_with_types`, `TypeTable` | 0.5 |
| `crates/saule-typeck/src/expr/infer.rs`, `expr.rs`, `stmt.rs` | record inferred types | 0.5 |
| `crates/saule-semantic/src/lib.rs` | `analyze_with_bindings`, `ResolveTable` | 0.6 |
| `crates/saule-semantic/src/resolve.rs`, `resolve/scope.rs`, `resolve/exprs.rs`, `resolve/decls.rs` | ordered frames, slot assignment, free-variable analysis | 0.6 |

### Deleted or superseded (Phase 4, not before)

| File | Superseded by |
|---|---|
| `crates/saule-interpreter/src/capture.rs` | Exact free-variable analysis in `saule-semantic` (0.6) — **can be removed in Phase 0** |
| `crates/saule-interpreter/src/env.rs` | Register frames + upvalues (§6, §7) |
| `crates/saule-interpreter/src/recycle.rs` | Register-range multi-return (§6.3) |
| `crates/saule-interpreter/src/eval/` (whole tree) | `saule-vm` |
| `thrown_slot` / `pending_flow` (`eval/stmt/mod.rs:36-72`) | Handler tables (§12.1), jump-to-exit for `match` (§9.4) |

### Untouched

`crates/saule-interpreter/src/stdlib/` (all of it), `native_packages.rs`,
`dynamic_packages/`, `native_host.rs`, `output.rs`, `platform.rs`, `error.rs`,
`value/table.rs`, `value/file.rs`, `value/interface.rs`, `module/`,
`saule-lexer`, `saule-parser`, `saule-fmt`, `saule-docs`, `saule-lsp`,
`saule-db`, `saule-sdk`, `saule-native-abi`, `saule-export-macro`,
`saule-project`, `saule-version`, `saule-engine-lib`.

### New

`crates/saule-vm/` — see §17.1 for the layout.

---

## Appendix B — worked compilation examples

### B.1 A tight arithmetic loop

```saule
fn sum(n: integer) -> integer
  local total: integer = 0
  for i = 1 to n do
    total = total + i
  end
  return total
end
```

Registers: `r0 = n` (parameter), `r1 = total`, `r2..r4` = loop control,
`r5 = i`.

```
        LOADI     r1  0             ; total = 0
        LOADI     r2  1             ; counter
        MOVE      r3  r0            ; limit = n
        LOADI     r4  1             ; step
        FORPREP_I r2  →exit
body:   ADDI      r1  r1  r5        ; total = total + i
        FORLOOP_I r2  →body
exit:   RET1      r1
```

**Six instructions total, three in the loop body.** No allocation, no hashing, no
refcount traffic — `ADDI` reads two `i64`s and writes an `i64`. Compare with the
current path: per iteration, a scope recycle, a hash insert to bind `i`, two
`Environment::get` chain walks for `total` and `i`, an `Environment::assign`
chain walk, and a `Value` clone at each step.

### B.2 A method call with a defaulted parameter

```saule
class Player
  health: integer = 100

  fn damage(amount: integer, critical: boolean = false) -> nil
    self.health = self.health - (critical and amount * 2 or amount)
  end
end

-- call site:
p.damage(10)
```

Layout: `Player.layout.index["health"] == 0`. Vtable: `damage` at slot 0.

Call site — one instruction plus argument setup:

```
        MOVE      r6  r2            ; receiver (p)
        LOADI     r7  10            ; amount
        CALLM     r6  2  0          ; vtable slot 0, 1 arg, result -> r6
```

`critical` is absent, so the compiler targets `damage`'s **1-argument entry
stub**, which materializes the default and falls through to the shared body
(§19).

Method body (`self` = `r0`, `amount` = `r1`, `critical` = `r2`):

```
stub1:  LOADBOOL  r2  0             ; critical = false
body:   TEST      r2  0             ; if not critical, skip
        JMP       →plain
        MULII     r3  r1  2         ; amount * 2
        JMP       →apply
plain:  MOVE      r3  r1
apply:  GETF      r4  r0  0         ; r4 := self.health
        SUBI      r4  r4  r3
        SETF      r0  0   r4        ; self.health := r4
        RET0
```

Every field access is an indexed load. The method is reached through one vtable
index. Nothing hashes a string; nothing allocates.

### B.3 A `match` over an enum

```saule
match event
  case Event.Click(x, y) then handleClick(x, y)
  case Event.Key(code)   then handleKey(code)
  case Event.Quit        then shutdown()
end
```

Tags: `Click = 0`, `Key = 1`, `Quit = 2`.

```
        GETTAG    r5  r4
        SWITCH    r5  T0            ; jump table: [→click, →key, →quit]

click:  UNWRAP    r6  r4            ; payload table
        GETARR    r7  r6  1
        GETARR    r8  r6  2
        CALLK     r7  3             ; handleClick(x, y) -- proto in EXTRAARG
        JMP       →done
key:    UNWRAP    r6  r4
        GETARR    r7  r6  1
        CALLK     r7  2
        JMP       →done
quit:   CALLK     r6  1
done:
```

One indexed jump regardless of arm count. The current implementation walks arms
in order, comparing `enum_name` and `variant_name` strings per arm.

---

*End of document.*
