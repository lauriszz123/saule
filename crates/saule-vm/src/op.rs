//! Instruction encoding — the opcode table from `VM_DESIGN.md` §15 and the
//! fixed-width 32-bit word layout from §5.2.
//!
//! ```text
//!  31       24 23       16 15        8 7         0
//! ┌───────────┬───────────┬───────────┬───────────┐
//! │    op     │     A     │     B     │     C     │   ABC
//! ├───────────┼───────────┼───────────┴───────────┤
//! │    op     │     A     │          Bx           │   ABx   (unsigned 16)
//! ├───────────┼───────────┼───────────────────────┤
//! │    op     │     A     │         sBx           │   AsBx  (biased signed 16)
//! ├───────────┼───────────┴───────────────────────┤
//! │    op     │              Ax                   │   Ax    (unsigned 24)
//! └───────────┴───────────────────────────────────┘
//! ```
//!
//! Decoding is shifts and masks. Every operand that needs more than its
//! field allows is carried by a following [`Op::EXTRAARG`] word, which the
//! dispatch loop reads inline and never executes on its own.
//!
//! **The opcode table is the ABI of a compiled chunk.** Adding an opcode at
//! the end is free; renumbering one invalidates every chunk ever written,
//! which matters the day the bytecode cache of §14 lands. Append, don't
//! insert.

use std::fmt;

/// Operand layout of an instruction word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fmt {
    /// Three 8-bit operands.
    Abc,
    /// One 8-bit operand plus an unsigned 16-bit operand.
    ABx,
    /// One 8-bit operand plus a signed 16-bit operand (jump displacement).
    AsBx,
    /// One unsigned 24-bit operand.
    Ax,
}

/// Bias applied to `sBx` so a signed displacement survives a `u16` field.
/// Gives a jump range of `-32768 ..= 32767` instructions.
pub const SBX_BIAS: i32 = 32768;

/// Largest register index a frame can name, from the 8-bit `A`/`B`/`C`
/// fields. Exceeding it is a `CompileError`, never a panic (§24.4).
pub const MAX_REGS: u16 = 256;

macro_rules! define_ops {
    ($( $(#[doc = $doc:expr])* $name:ident : $fmt:ident ),* $(,)?) => {
        /// Every opcode in the instruction set. Dense and `#[repr(u8)]` so
        /// the dispatch `match` lowers to a jump table (§5.3).
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[allow(non_camel_case_types)]
        #[repr(u8)]
        pub enum Op {
            $( $(#[doc = $doc])* $name, )*
        }

        impl Op {
            /// All opcodes, in encoding order. Index equals discriminant.
            pub const ALL: &'static [Op] = &[ $( Op::$name, )* ];

            /// Decode an opcode byte. `None` for an unassigned value — the
            /// verifier rejects those rather than the dispatch loop
            /// transmuting blindly.
            #[inline]
            pub fn from_u8(v: u8) -> Option<Op> {
                Self::ALL.get(v as usize).copied()
            }

            /// Mnemonic, as printed by the disassembler.
            pub fn name(self) -> &'static str {
                match self { $( Op::$name => stringify!($name), )* }
            }

            /// Operand layout, which is all the disassembler needs to know
            /// to print any instruction.
            pub fn fmt(self) -> Fmt {
                match self { $( Op::$name => Fmt::$fmt, )* }
            }

            /// Whether this opcode's `sBx` operand is a **jump
            /// displacement** rather than an inline value.
            ///
            /// `LOADI` and `LOADF` share the `AsBx` layout but carry a
            /// literal, so format alone cannot tell the disassembler which
            /// operands to resolve to a target.
            pub fn is_jump(self) -> bool {
                matches!(
                    self,
                    Op::JMP
                        | Op::FORPREP_I
                        | Op::FORLOOP_I
                        | Op::FORPREP_F
                        | Op::FORLOOP_F
                        | Op::ITERNEXT
                )
            }
        }
    };
}

define_ops! {
    // ---- §15.1 Moves and constants ------------------------------------
    /// `R[A] := R[B]`
    MOVE: Abc,
    /// `R[A] := K[Bx]`
    LOADK: ABx,
    /// `R[A] := Int(sBx)` — small integer literals inline
    LOADI: AsBx,
    /// `R[A] := Float(sBx as f64)` — whole-number float literals
    LOADF: AsBx,
    /// `R[A] := Bool(B != 0)`
    LOADBOOL: Abc,
    /// `R[A] ..= R[A+B] := Nil`
    LOADNIL: Abc,
    /// Never executed; supplies a 24-bit operand to the preceding instruction
    EXTRAARG: Ax,

    // ---- §15.2 Upvalues, module slots, statics ------------------------
    /// `R[A] := U[B]`
    GETUPVAL: Abc,
    /// `U[B] := R[A]`
    SETUPVAL: Abc,
    /// Close every open upvalue pointing at a register >= A
    CLOSEUP: Abc,
    /// `R[A] := M[Bx]`
    GETMOD: ABx,
    /// `M[Bx] := R[A]`
    SETMOD: ABx,
    /// `R[A] := ClassProto[B].statics[C]`
    GETSTAT: Abc,
    /// `ClassProto[B].statics[C] := R[A]`
    SETSTAT: Abc,
    /// `R[A] := new Closure(P[Bx])`, binding upvalues per `P[Bx].upvals`
    CLOSURE: ABx,

    // ---- §15.3 Integer arithmetic (wrapping, per ops.rs) --------------
    /// `R[A] := R[B] + R[C]`, wrapping
    ADDI: Abc,
    /// `R[A] := R[B] - R[C]`, wrapping
    SUBI: Abc,
    /// `R[A] := R[B] * R[C]`, wrapping
    MULI: Abc,
    /// `R[A] := R[B] / R[C]`; zero divisor is `DivisionByZero`
    DIVI: Abc,
    /// `R[A] := R[B] % R[C]`; zero divisor is `DivisionByZero`
    MODI: Abc,
    /// `R[A] := R[B] ^ R[C]`; negative exponent is an error
    POWI: Abc,
    /// `R[A] := -R[B]`
    NEGI: Abc,
    /// `R[A] := R[B] + sext(C)` — signed 8-bit immediate
    ADDII: Abc,
    /// `R[A] := R[B] - sext(C)`
    SUBII: Abc,
    /// `R[A] := R[B] * sext(C)`
    MULII: Abc,

    // ---- §15.4 Float arithmetic ---------------------------------------
    /// `R[A] := R[B] + R[C]`, IEEE 754
    ADDF: Abc,
    /// `R[A] := R[B] - R[C]`, IEEE 754
    SUBF: Abc,
    /// `R[A] := R[B] * R[C]`, IEEE 754
    MULF: Abc,
    /// `R[A] := R[B] / R[C]`, IEEE 754 (zero divisor yields infinity)
    DIVF: Abc,
    /// `R[A] := R[B] % R[C]`, IEEE 754
    MODF: Abc,
    /// `R[A] := R[B] ^ R[C]`
    POWF: Abc,
    /// `R[A] := -R[B]`
    NEGF: Abc,

    // ---- §15.5 Bitwise (integer only) ---------------------------------
    /// `R[A] := R[B] & R[C]`
    BAND: Abc,
    /// `R[A] := R[B] | R[C]`
    BOR: Abc,
    /// `R[A] := R[B] ~ R[C]`
    BXOR: Abc,
    /// `R[A] := R[B] << R[C]`, saturating past 64
    SHL: Abc,
    /// `R[A] := R[B] >> R[C]`, saturating past 64
    SHR: Abc,
    /// `R[A] := ~R[B]`
    BNOT: Abc,

    // ---- §15.6 Dynamic arithmetic fallback ----------------------------
    /// `R[A] := binary(op from EXTRAARG, R[B], R[C])` — the full
    /// `ops::binary` path including `Op*` overload dispatch
    ARITHX: Abc,
    /// `R[A] := unary(op from EXTRAARG, R[B])`
    UNARYX: Abc,

    // ---- §15.7 Comparison and branching -------------------------------
    /// `pc += sBx`; if `A > 0`, first `CLOSEUP` at register `A-1`
    JMP: AsBx,
    /// If `R[A] <  R[B]` as `i64`, skip the next instruction
    JLTI: Abc,
    /// If `R[A] <= R[B]` as `i64`, skip the next instruction
    JLEI: Abc,
    /// If `R[A] >  R[B]` as `i64`, skip the next instruction
    JGTI: Abc,
    /// If `R[A] >= R[B]` as `i64`, skip the next instruction
    JGEI: Abc,
    /// If `R[A] <  R[B]` as `f64`, skip the next instruction
    JLTF: Abc,
    /// If `R[A] <= R[B]` as `f64`, skip the next instruction
    JLEF: Abc,
    /// If `R[A] >  R[B]` as `f64`, skip the next instruction
    JGTF: Abc,
    /// If `R[A] >= R[B]` as `f64`, skip the next instruction
    JGEF: Abc,
    /// If `R[A] == R[B]` as `i64`, skip the next instruction
    JEQI: Abc,
    /// If `R[A] != R[B]` as `i64`, skip the next instruction
    JNEI: Abc,
    /// If `values_equal(R[A], R[B])`, skip the next instruction
    JEQ: Abc,
    /// If `!values_equal(R[A], R[B])`, skip the next instruction
    JNE: Abc,
    /// If `R[A] == K[C]`, skip the next instruction — `match` chains
    JEQK: Abc,
    /// If `R[A].is_truthy() != (C != 0)`, skip the next instruction
    TEST: Abc,
    /// `and`/`or`: if truthiness matches `C`, `R[A] := R[B]` and skip
    TESTSET: Abc,
    /// If `R[A]` is nil, skip the next instruction
    JNIL: Abc,
    /// If `R[A]` is not nil, skip the next instruction
    JNOTNIL: Abc,
    /// `R[A] := Bool(R[B] <  R[C])`, integer
    LTI: Abc,
    /// `R[A] := Bool(R[B] <= R[C])`, integer
    LEI: Abc,
    /// `R[A] := Bool(R[B] == R[C])`, integer
    EQI: Abc,
    /// `R[A] := Bool(R[B] <  R[C])`, float
    LTF: Abc,
    /// `R[A] := Bool(R[B] <= R[C])`, float
    LEF: Abc,
    /// `R[A] := Bool(R[B] == R[C])`, float
    EQF: Abc,
    /// `R[A] := Bool(values_equal(R[B], R[C]))`
    EQV: Abc,
    /// `R[A] := Bool(!R[B].is_truthy())`
    NOT: Abc,

    // ---- §15.8 Loops ---------------------------------------------------
    /// Validate integer counter/limit/step in `R[A]..R[A+2]`; jump `sBx`
    /// when the body will not run; else seed the user variable `R[A+3]`
    FORPREP_I: AsBx,
    /// Step the integer loop; jump back `sBx` while in range
    FORLOOP_I: AsBx,
    /// Float variant of `FORPREP_I`
    FORPREP_F: AsBx,
    /// Float variant of `FORLOOP_I`
    FORLOOP_F: AsBx,
    /// Resolve the iteration source in `R[A]` into control state
    /// `R[A]..R[A+2]`; jump `Bx` when empty
    ITERPREP: ABx,
    /// Advance the iterator; write key/value into `R[A+3]`/`R[A+4]` and
    /// jump back `sBx` while values remain
    ITERNEXT: AsBx,

    // ---- §15.9 Tables --------------------------------------------------
    /// `R[A] := new table`, array capacity hint `B`, map capacity hint `C`
    NEWT: Abc,
    /// Append `R[A+1]..R[A+B]` to `R[A]`'s array part in bulk
    SETLIST: Abc,
    /// `R[A] := R[B].array[R[C] - 1]`, bounds-checked, 1-based
    GETARR: Abc,
    /// `R[A].array[R[B] - 1] := R[C]`
    SETARR: Abc,
    /// `R[A] := R[B].map[key(R[C])]`
    GETMAP: Abc,
    /// `R[A].map[key(R[B])] := R[C]`
    SETMAP: Abc,
    /// `R[A] := R[B].map[K[C]]`, constant key
    GETMAPK: Abc,
    /// `R[A].map[K[B]] := R[C]`
    SETMAPK: Abc,
    /// Fully dynamic index read — receiver type unknown
    GETIDX: Abc,
    /// Fully dynamic index write
    SETIDX: Abc,
    /// Push `R[B]` onto `R[A]`'s array part
    APPEND: Abc,
    /// `R[A] := #R[B]`
    LEN: Abc,

    // ---- §15.10 Classes and instances ----------------------------------
    /// `R[A] := new instance of ClassProto[Bx]`
    NEW: ABx,
    /// `R[A] := R[B].fields[C]` — static slot
    GETF: Abc,
    /// `R[A].fields[B] := R[C]` — static slot
    SETF: Abc,
    /// `R[A] := R[B].<K[C]>` via inline cache
    GETFX: Abc,
    /// `R[A].<K[B]> := R[C]` via inline cache
    SETFX: Abc,
    /// Method call: `R[A]` receiver, args `R[A+1]..R[A+B-1]`, vtable slot
    /// `C`, one result into `R[A]`
    CALLM: Abc,
    /// Multi-return method call; vtable slot in `EXTRAARG`
    CALLM_MR: Abc,
    /// Interface dispatch; interface index in `EXTRAARG`
    CALLIF: Abc,
    /// Static method call; class and slot in `EXTRAARG`
    CALLSTAT: Abc,
    /// `self.super(args)` — dispatch to the parent's `init`
    SUPER: Abc,
    /// `R[A] := Bool(R[B] is a ClassProto[C] or a subclass)`
    ISA: Abc,

    // ---- §15.11 Enums and `match` --------------------------------------
    /// `R[A] := Int(tag of the enum variant in R[B])`
    GETTAG: Abc,
    /// Jump through jump table `Bx` indexed by `R[A]`
    SWITCH: ABx,
    /// If `R[A]`'s tag == B, skip the next instruction
    JIFTAG: Abc,
    /// `R[A] := EnumProto[Bx].by_tag[C]` — singleton variant
    VARIANT: ABx,
    /// Construct a tuple variant from `R[A+1]..R[A+B-1]`
    NEWVAR: Abc,
    /// `R[A] := payload of the variant in R[B]`
    UNWRAP: Abc,

    // ---- §15.12 Nullability ---------------------------------------------
    /// `R[A] := if R[B] is nil { R[C] } else { R[B] }`
    COALESCE: Abc,
    /// `x!` — `R[A] := R[B]`, or `ForceUnwrapNil`
    UNWRAPNIL: Abc,
    /// `x as T` — `R[A] := R[B]` if `R[B]` matches `cast_types[C]`, else nil
    CASTCHK: Abc,

    // ---- §15.13 Calls and returns ---------------------------------------
    /// `R[A]` callee, args `R[A+1]..R[A+B-1]` (`B=0`: to top), results into
    /// `R[A]..R[A+C-2]` (`C=0`: all, set top)
    CALL: Abc,
    /// Statically resolved callee; proto index in `EXTRAARG`
    CALLK: Abc,
    /// Native call; constant index of the native in `EXTRAARG`
    CALLNAT: Abc,
    /// `return f(args)` in tail position: **replace** the running frame
    /// rather than nest inside it. Callee in `R[A]`, args
    /// `R[A+1]..R[A+B-1]`, which move down to `base`.
    ///
    /// The callee's results go to whoever called the frame being replaced,
    /// so `ret_to` and `n_ret` are inherited and multi-return keeps working
    /// through a tail chain for free.
    ///
    /// Dispatches like `CALL`: only a bytecode closure has a frame to
    /// replace. A native, a constructor or anything else callable is an
    /// ordinary call made here and returned — which is exactly the line the
    /// tree-walker draws, since it builds `Flow::TailCall` only for a
    /// `Value::Function`.
    TAILCALL: Abc,
    /// Return `R[A]..R[A+B-2]`; `B=0` returns to top
    RET: Abc,
    /// Return no values
    RET0: Abc,
    /// Return `R[A]` — by far the most common shape
    RET1: Abc,

    // ---- §15.14 Strings --------------------------------------------------
    /// `R[A] := R[B] .. R[B+1] .. … .. R[C]` — n-ary, one allocation
    CONCAT: Abc,
    /// `R[A] := display(R[B])`, dispatching `OpToString` for instances
    TOSTR: Abc,

    // ---- §15.15 Errors ---------------------------------------------------
    /// Set the pending value from `R[A]` and unwind to the nearest handler
    THROW: Abc,
    /// `R[A] := Bool(R[B] matches type descriptor C)` — used by `catch`
    CHKTY: Abc,

    // ---- §19 variadic parameters -----------------------------------------
    /// `R[A] := table of R[A] .. R[n_args)` — gather the surplus arguments
    /// of a variadic callee.
    ///
    /// Emitted as the callee's **first** instruction, so it runs however the
    /// frame was entered. Done in the callee rather than by packing a table
    /// at the call site, which would have needed no new opcode but only
    /// works where the caller can *see* that the callee is variadic — not
    /// through a function value, and not across a module boundary. A callee
    /// that gathers its own arguments is right for every call.
    VARARG: Abc,

    // ---- §8.5 dynamic member dispatch ------------------------------------
    /// `R[A] := R[A].<K in EXTRAARG>(R[A+1] … R[A+B-1])`, `C` results.
    ///
    /// The dynamic counterpart to `CALLM`, for a receiver whose class the
    /// front end did not prove. Defers to the tree-walker's own
    /// `dispatch_member_call_multi`, so every receiver kind — instances,
    /// classes, enums, file handles — behaves identically by construction
    /// rather than by the compiler learning each one separately.
    CALLMX: Abc,

    // ---- §15.8 dynamic generic iteration ---------------------------------
    /// Resolve an **unproved** iteration source in `R[A]` into control state
    /// `R[A]..R[A+2]`; jump `Bx` when a table source is empty.
    ///
    /// The dynamic counterpart to `ITERPREP`, and it **dispatches** rather
    /// than normalises. A table and a closure driver do not share a
    /// termination rule — the driver stops on a nil, the table snapshot has
    /// no terminator and walks every pair — so folding both into one
    /// protocol cannot be done without losing that distinction. Saule's
    /// `t[i] = nil` *stores* a nil rather than deleting the key (unlike
    /// Lua), so a table really can hold one, and a one-variable loop binds
    /// the **value**: normalising a table into a nil-terminated driver would
    /// stop such a loop early here and run it to completion under the
    /// tree-walker. That is why `R[A+2]` carries a mode flag and the
    /// compiler emits both steps behind a `TEST`, mirroring the
    /// tree-walker's own runtime `match` on the source value.
    ///
    /// * table → `R[A]` := the snapshot, `R[A+1]` := 0, `R[A+2]` := false
    /// * function → `R[A]` keeps the driver, `R[A+2]` := true
    /// * instance → `iter()` runs here, once per loop, and its result
    ///   replaces `R[A]`; `R[A+2]` := true
    ITERPREPX: ABx,

    // ---- §6.4 tail calls, statically resolved ----------------------------
    /// `TAILCALL` to a statically known proto; module and proto packed 8/16
    /// in `EXTRAARG`, args at `R[A]..R[A+B-2]` — `CALLK`'s layout, since
    /// there is no callee register when the callee is the operand.
    ///
    /// Its own opcode rather than a flag on `CALLK` because the frame is
    /// *replaced* rather than pushed, which is a different thing for the
    /// dispatch loop to do, and because a `C` operand it does not have is
    /// where a flag would have to live.
    TAILCALLK: Abc,
    /// `TAILCALL` to a static method; declaring class and slot packed 8/16
    /// in `EXTRAARG`, args at `R[A]..R[A+B-2]` — `CALLSTAT`'s layout.
    ///
    /// Needed as well as `TAILCALLK` because a static method's proto is
    /// reached through the class table at run time, not named directly.
    /// `class Main` / `static fn` is the idiomatic shape of a Saule program,
    /// so without this the commonest tail-recursive function in the language
    /// would still grow the frame stack.
    TAILCALLS: Abc,

    // ---- §9 tuple patterns -----------------------------------------------
    /// `R[A] := how many values the variadic window at `R[B]` holds`, as an
    /// integer — `top - (base + B)`, saturating at 0.
    ///
    /// A tuple pattern's arity test needs the *count*, not the values: the
    /// oracle refuses to match `case (q, r, s)` against a two-value
    /// scrutinee, so a compiler that padded the window with nil and skipped
    /// the test would match an arm the tree-walker rejects. The count is only
    /// knowable at run time for a call, since `top` is what the callee set.
    ///
    /// Emitted once per `match` whose scrutinee is a call and whose arms use
    /// a tuple pattern, never on any other path.
    NVALS: Abc,

    // ---- §7 self-recursive lambdas ---------------------------------------
    /// `R[A] := the closure this frame is running`
    ///
    /// `local go = fn(k) … go(k - 1) … end` — a lambda that calls itself by
    /// the name it is being bound to. Capturing that name as an upvalue
    /// would work and would **leak**: the closed cell holds the closure and
    /// the closure holds the cell, an `Rc` cycle per call. The tree-walker
    /// solved the same problem by *not* capturing — `FunctionObject`'s
    /// `self_name` resolves the recursion through the call scope — and this
    /// is the bytecode counterpart: the running closure is already on the
    /// frame, so the recursive call reads it from there and no cell exists
    /// to close a cycle with.
    ///
    /// Appended, never inserted: the numbering is the chunk ABI.
    SELFFUNC: Abc,

    // ---- §16 superinstructions -------------------------------------------
    /// `x as T` immediately force-unwrapped: `R[A] := R[B]` if `R[B]` matches
    /// `cast_types[C]`, else `ForceUnwrapNil`.
    ///
    /// **The first superinstruction in this instruction set, and the only
    /// candidate a profile has ever supported.** §16 says every one must be
    /// justified by a measured opcode-pair histogram before it is added;
    /// `--profile-bytecode` counts `CASTCHK UNWRAPNIL` as an adjacent pair
    /// **6,665,964 times** in `benchmarks/sau/sort.sau` — 22.9% of that
    /// program in each half, 46% together. Nothing else in any benchmark
    /// comes close, and every candidate the task list was originally written
    /// with (`GETF_CALLM`, `FORLOOP_GETARR`, `ADDII_MOVE`, …) is unsupported
    /// by any reading.
    ///
    /// Read the caveat with the number: `sort` spends that time because its
    /// comparator writes `(a as integer)!` on an untyped parameter, and the
    /// tree-walker does the same work. The *pair* is a compiler artifact and
    /// fusing it removes one dispatch and one register write per comparison;
    /// the *cast* is the program's own semantics and is still performed.
    ///
    /// Appended after `SELFFUNC`, never inserted: the numbering is the chunk
    /// ABI.
    CASTUNWRAP: Abc,
}

/// The operator an `ARITHX` / `UNARYX` carries in its `EXTRAARG`.
///
/// An **explicit** numbering, not `BinOp`'s discriminants. Those are an
/// implementation detail of `saule-ast` that a refactor could renumber
/// without anyone noticing — and this value is part of the chunk ABI, so a
/// silent renumbering would turn every cached `+` into a `-`.
pub mod dynop {
    use saule_ast::{BinOp, UnaryOp};

    pub const ADD: u32 = 0;
    pub const SUB: u32 = 1;
    pub const MUL: u32 = 2;
    pub const DIV: u32 = 3;
    pub const MOD: u32 = 4;
    pub const POW: u32 = 5;
    pub const BAND: u32 = 6;
    pub const BOR: u32 = 7;
    pub const BXOR: u32 = 8;
    pub const SHL: u32 = 9;
    pub const SHR: u32 = 10;
    pub const EQ: u32 = 11;
    pub const NOTEQ: u32 = 12;
    pub const LT: u32 = 13;
    pub const LTEQ: u32 = 14;
    pub const GT: u32 = 15;
    pub const GTEQ: u32 = 16;
    pub const CONCAT: u32 = 17;

    pub const NEG: u32 = 0;
    pub const NOT: u32 = 1;
    pub const LEN: u32 = 2;
    pub const BNOT: u32 = 3;

    /// `None` for the short-circuiting operators: they are control flow and
    /// never reach a dynamic arithmetic opcode.
    pub fn encode_binary(op: BinOp) -> Option<u32> {
        Some(match op {
            BinOp::Add => ADD,
            BinOp::Sub => SUB,
            BinOp::Mul => MUL,
            BinOp::Div => DIV,
            BinOp::Mod => MOD,
            BinOp::Pow => POW,
            BinOp::BAnd => BAND,
            BinOp::BOr => BOR,
            BinOp::BXor => BXOR,
            BinOp::Shl => SHL,
            BinOp::Shr => SHR,
            BinOp::Eq => EQ,
            BinOp::NotEq => NOTEQ,
            BinOp::Lt => LT,
            BinOp::LtEq => LTEQ,
            BinOp::Gt => GT,
            BinOp::GtEq => GTEQ,
            BinOp::Concat => CONCAT,
            BinOp::And | BinOp::Or | BinOp::Coalesce => return None,
        })
    }

    pub fn decode_binary(v: u32) -> Option<BinOp> {
        Some(match v {
            ADD => BinOp::Add,
            SUB => BinOp::Sub,
            MUL => BinOp::Mul,
            DIV => BinOp::Div,
            MOD => BinOp::Mod,
            POW => BinOp::Pow,
            BAND => BinOp::BAnd,
            BOR => BinOp::BOr,
            BXOR => BinOp::BXor,
            SHL => BinOp::Shl,
            SHR => BinOp::Shr,
            EQ => BinOp::Eq,
            NOTEQ => BinOp::NotEq,
            LT => BinOp::Lt,
            LTEQ => BinOp::LtEq,
            GT => BinOp::Gt,
            GTEQ => BinOp::GtEq,
            CONCAT => BinOp::Concat,
            _ => return None,
        })
    }

    pub fn encode_unary(op: UnaryOp) -> u32 {
        match op {
            UnaryOp::Neg => NEG,
            UnaryOp::Not => NOT,
            UnaryOp::Len => LEN,
            UnaryOp::BNot => BNOT,
        }
    }

    pub fn decode_unary(v: u32) -> Option<UnaryOp> {
        Some(match v {
            NEG => UnaryOp::Neg,
            NOT => UnaryOp::Not,
            LEN => UnaryOp::Len,
            BNOT => UnaryOp::BNot,
            _ => return None,
        })
    }
}

/// One encoded instruction word.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Instruction(pub u32);

impl Instruction {
    /// The opcode byte, undecoded. `op()` is the checked form.
    #[inline]
    pub const fn raw_op(self) -> u8 {
        (self.0 >> 24) as u8
    }

    /// Decoded opcode, or `None` if the byte is not an assigned opcode.
    #[inline]
    pub fn op(self) -> Option<Op> {
        Op::from_u8(self.raw_op())
    }

    #[inline]
    pub const fn a(self) -> u8 {
        (self.0 >> 16) as u8
    }

    #[inline]
    pub const fn b(self) -> u8 {
        (self.0 >> 8) as u8
    }

    #[inline]
    pub const fn c(self) -> u8 {
        self.0 as u8
    }

    /// `C` read as a signed 8-bit immediate (the `ADDII` family).
    #[inline]
    pub const fn sc(self) -> i64 {
        (self.0 as u8) as i8 as i64
    }

    #[inline]
    pub const fn bx(self) -> u16 {
        self.0 as u16
    }

    /// Signed jump displacement, un-biased.
    #[inline]
    pub const fn sbx(self) -> i32 {
        (self.0 as u16) as i32 - SBX_BIAS
    }

    #[inline]
    pub const fn ax(self) -> u32 {
        self.0 & 0x00ff_ffff
    }

    pub const fn abc(op: Op, a: u8, b: u8, c: u8) -> Instruction {
        Instruction(((op as u32) << 24) | ((a as u32) << 16) | ((b as u32) << 8) | c as u32)
    }

    pub const fn abx(op: Op, a: u8, bx: u16) -> Instruction {
        Instruction(((op as u32) << 24) | ((a as u32) << 16) | bx as u32)
    }

    /// Build an `AsBx` word.
    ///
    /// Panics on an out-of-range operand, and does so in **release too** —
    /// this asserts rather than `debug_assert`s on purpose. A silent 16-bit
    /// truncation here produces a chunk that runs and computes the wrong
    /// answer, which is the single worst failure mode this crate can have.
    /// Emitters that might legitimately overflow (a far jump needing a
    /// trampoline, a literal too large for `LOADI`) must range-check first —
    /// see [`try_asbx`](Instruction::try_asbx) — and turn the miss into a
    /// `CompileError` or a `LOADK`.
    pub fn asbx(op: Op, a: u8, sbx: i32) -> Instruction {
        Instruction::try_asbx(op, a, sbx)
            .unwrap_or_else(|| panic!("{op} operand {sbx} does not fit in sBx"))
    }

    /// Fallible `AsBx` constructor. `None` means the operand does not fit and
    /// the caller must pick another encoding.
    pub fn try_asbx(op: Op, a: u8, sbx: i32) -> Option<Instruction> {
        if !(-SBX_BIAS..SBX_BIAS).contains(&sbx) {
            return None;
        }
        let biased = (sbx + SBX_BIAS) as u32 & 0xffff;
        Some(Instruction(((op as u32) << 24) | ((a as u32) << 16) | biased))
    }

    pub const fn ax_of(op: Op, ax: u32) -> Instruction {
        Instruction(((op as u32) << 24) | (ax & 0x00ff_ffff))
    }
}

impl fmt::Debug for Instruction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.op() {
            Some(op) => write!(f, "{}({:#010x})", op.name(), self.0),
            None => write!(f, "<bad op {:#04x}>({:#010x})", self.raw_op(), self.0),
        }
    }
}

impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opcodes_are_dense_and_round_trip() {
        for (i, op) in Op::ALL.iter().enumerate() {
            assert_eq!(Op::from_u8(i as u8), Some(*op), "gap at discriminant {i}");
            assert_eq!(*op as u8 as usize, i);
        }
        assert!(Op::ALL.len() <= 256);
        assert_eq!(Op::from_u8(Op::ALL.len() as u8), None);
    }

    #[test]
    fn abc_round_trips() {
        let ins = Instruction::abc(Op::ADDI, 3, 200, 255);
        assert_eq!(ins.op(), Some(Op::ADDI));
        assert_eq!((ins.a(), ins.b(), ins.c()), (3, 200, 255));
        assert_eq!(ins.sc(), -1);
    }

    #[test]
    fn abx_round_trips() {
        let ins = Instruction::abx(Op::LOADK, 7, 65535);
        assert_eq!(ins.op(), Some(Op::LOADK));
        assert_eq!((ins.a(), ins.bx()), (7, 65535));
    }

    #[test]
    fn asbx_round_trips_across_the_range() {
        for sbx in [-32768, -1, 0, 1, 32767] {
            let ins = Instruction::asbx(Op::JMP, 0, sbx);
            assert_eq!(ins.op(), Some(Op::JMP));
            assert_eq!(ins.sbx(), sbx, "sBx {sbx} did not round-trip");
        }
    }

    #[test]
    fn ax_round_trips() {
        let ins = Instruction::ax_of(Op::EXTRAARG, 0x00ff_ffff);
        assert_eq!(ins.op(), Some(Op::EXTRAARG));
        assert_eq!(ins.ax(), 0x00ff_ffff);
    }
}
