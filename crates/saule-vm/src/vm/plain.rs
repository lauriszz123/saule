//! Numeric loops, run on numbers.
//!
//! A register holds a `Value`: a tag and a payload. An instruction of the
//! main loop that adds two integers reads two tags to see that they are
//! integers, adds, looks at what the destination holds in case it has to
//! be dropped, and writes a tag and a payload: thirty machine instructions
//! with the fetch and the jump, of which one is the addition. In a loop
//! whose body is nothing but arithmetic none of that can ever find
//! anything: the registers hold integers when the loop starts and nothing
//! in it can make them anything else.
//!
//! So such a loop is run here instead. When a loop starts whose body only
//! reads and writes registers and the elements of tables ([`Plan::of`]),
//! the registers it reads are looked at once, their numbers are copied
//! into an array of plain 64-bit words, the body runs on that array
//! ([`Plan::run`]), and what it wrote is copied back as values when it
//! leaves. The body is decoded once, when the plan is made.
//!
//! This is also the way out `PERF_TASKS.md` names for the main loop, whose
//! arms cannot be made cheaper from inside it: a hot loop that is not that
//! function.
//!
//! ## Which loops
//!
//! * A numeric `for`, from the instruction after its `FORPREP` to its
//!   `FORLOOP`: handed over by the main loop's `FORPREP`, when the turns
//!   it has to make are enough to pay for the handing over
//!   ([`Plan::worth`]).
//! * Any loop that ends in a jump back, a `while` above all: from where
//!   the jump lands to the jump. There is nothing at its head to hand it
//!   over from, so the main loop does it at the jump back, once the loop
//!   has gone round a few times ([`Plan::due`]).
//!
//! ## It cannot do anything the main loop would not
//!
//! Each step here does to its words what the main loop's arm does to its
//! registers when that arm succeeds. Whenever the main loop would do
//! anything else, this stops *before* the instruction, copies back, and
//! returns its place: a division by zero, a `for` with a step of zero, an
//! index outside a table's array, an element that is not the kind the
//! loop goes on to use it as. The main loop then executes that
//! instruction, and the error, or the unusual case, is its own. A
//! register that does not hold what the body takes it for is found before
//! the first instruction, and the loop is left to the main loop whole.
//!
//! Nothing in a plan calls, so nothing else runs while its registers are
//! out of date: no closure that has captured one, no coroutine, no
//! destructor that could look. A value a register held before the loop,
//! and that the loop writes over, is dropped when the loop leaves and not
//! at the first write; nothing can tell. A table is read and written
//! through the same borrow and the same array the main loop uses, so a
//! `for … in` going over it elsewhere still gets its copy at the first
//! write.
//!
//! ## What a plan needs
//!
//! * Every instruction of the loop is one of the kinds below, loops
//!   inside it included.
//! * Every read finds the kind it reads. A register may be an integer in
//!   one place and a float in another, as the compiler's temporaries are;
//!   what is followed, forwards through every path, is what each register
//!   can hold at each instruction. A register read where it could hold
//!   either, or something the loop did not write and another read takes
//!   for something else, leaves the loop to the main loop.
//! * Every register that may be read before the loop writes it holds the
//!   kind it is read as when the loop starts.
//! * What `t[i]!` gives is of one kind wherever its register is read: the
//!   bytecode does not say what a table holds, the reads of the result do.
//! * No more than [`MOST`] registers.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use saule_runtime::value::{TableObject, Value};

use super::ops::{float_in_range, int_in_range, jump, put};
use crate::op::{Instruction, Op};

/// The most registers a plan may use: what its arrays are made for.
/// Clearing them is part of what handing a loop over costs, and a loop of
/// arithmetic over more variables than this is not written by hand.
const MOST: usize = 64;

/// A table, as a plan holds one.
type Table = Rc<RefCell<TableObject>>;

/// What a register holds at some instruction of a loop.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
enum Kind {
    Int = 1,
    Float = 2,
    Bool = 3,
    Table = 4,
}

impl Kind {
    /// This kind among what a register can hold at some instruction: a
    /// bit for each kind, beside the three below.
    fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// What a register held when the loop started, among what it can hold.
const AS_IT_WAS: u8 = 1;
/// An element read out of a table, before any read of it has said what
/// kind it is taken for.
const ELEMENT: u8 = 0x40;
/// A copy of what another register held when the loop started, before
/// any read has said what that was.
const UNKNOWN: u8 = 0x80;

/// One instruction of a plan: the main loop's, with its operands taken
/// apart once. `a`, `b` and `c` are registers, by their place in the
/// plan's arrays; `n` is the immediate, the constant's bits, or where a
/// jump goes.
#[derive(Clone, Copy)]
struct Step {
    does: Does,
    a: u8,
    b: u8,
    c: u8,
    n: i64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Does {
    Move,
    /// `R[a] = n`, the bits of a number or a boolean, of the kind `c`.
    Load,
    AddI,
    SubI,
    MulI,
    DivI,
    ModI,
    NegI,
    AddII,
    SubII,
    MulII,
    AddF,
    SubF,
    MulF,
    DivF,
    NegF,
    /// An integer as a float, and a float cut off to an integer: `CONV`.
    ToFloat,
    ToInt,
    /// Go to step `n`.
    Jump,
    /// Leave, for the instruction at `n`.
    Leave,
    // Tests: the next step is skipped when the test holds.
    JltI,
    JleI,
    JgeI,
    JgtI,
    JeqI,
    JneI,
    JltII,
    JleII,
    JgtII,
    JgeII,
    JeqII,
    JneII,
    JltF,
    JleF,
    JgtF,
    JgeF,
    /// Skip unless `R[a]` is `c`.
    Test,
    Not,
    LtI,
    LeI,
    EqI,
    LtF,
    LeF,
    EqF,
    // The numeric `for`: registers `a` to `a + 3`, and `n` the step to go
    // to (out of the loop for a `Prep`, back into it for a `Loop`).
    PrepI,
    LoopI,
    PrepF,
    LoopF,
    // `R[a] = R[b][R[c]]`, when the element is there and of the kind the
    // step is for.
    GetI,
    GetF,
    GetB,
    GetT,
    /// `R[a][R[b]] = R[c]`, a number or a boolean into the array.
    Put,
    /// `R[a] = #R[b]`, of a table.
    Len,
}

impl Does {
    /// Which of `a`, `b` and `c` are registers.
    fn registers(self) -> [bool; 3] {
        use Does::*;
        match self {
            Jump | Leave => [false, false, false],
            Load | JltII | JleII | JgtII | JgeII | JeqII | JneII | Test => [true, false, false],
            // The `for` steps name four registers by the first.
            PrepI | LoopI | PrepF | LoopF => [true, false, false],
            Move | NegI | NegF | AddII | SubII | MulII | ToFloat | ToInt | Not | Len => {
                [true, true, false]
            }
            JltI | JleI | JgeI | JgtI | JeqI | JneI | JltF | JleF | JgtF | JgeF => {
                [true, true, false]
            }
            AddI | SubI | MulI | DivI | ModI | AddF | SubF | MulF | DivF | LtI | LeI | EqI
            | LtF | LeF | EqF | GetI | GetF | GetB | GetT | Put => [true, true, true],
        }
    }
}

/// A loop made ready to run on numbers.
pub(crate) struct Plan {
    /// The instruction the first step stands for.
    first: usize,
    /// One step for each instruction of the loop, in place, and two
    /// `Leave`s after the last.
    steps: Vec<Step>,
    /// The registers the loop touches, in order: a step names one by its
    /// place here.
    used: Vec<u8>,
    /// The ones that may be read before the loop writes them, by their
    /// place, and the kind each is read as: what they have to hold when it
    /// starts.
    taken: Vec<(u8, Kind)>,
    /// The ones that may be stored into a table as they were when the
    /// loop started, with nothing saying what kind that is: taken when
    /// they hold a number or a boolean, and the store left to the main
    /// loop when they do not.
    stored: Vec<u8>,
    /// The ones the loop may write: copied back when it leaves, each as
    /// the kind it last was.
    written: Vec<u8>,
    /// How often the main loop has come to this loop's jump back since
    /// the plan last ran: see [`Plan::due`].
    came: Cell<u8>,
}

/// The registers an instruction reads and writes, by the kind it takes
/// each for.
struct Operands {
    reads: [Option<(u8, Kind)>; 3],
    writes: [Option<(u8, Kind)>; 2],
    /// On which of its ways on the instruction writes: a `for` writes its
    /// variable only when it goes round again.
    on: On,
    /// The two registers of a `MOVE`, to and from: the kind is whatever
    /// the second holds.
    moved: Option<(u8, u8)>,
    /// A register written with an element of a table, whose kind is what
    /// it is read as.
    element: Option<u8>,
    /// A register read whatever it holds: what is stored into a table.
    whatever: Option<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum On {
    Both,
    /// Only when it goes on to the next step.
    Next,
    /// Only when it goes to the step it names.
    Named,
}

/// What `ins` reads and writes, or `None` for an instruction a plan
/// cannot hold. `casts` are the chunk's cast types, for `CONV`.
fn operands(ins: Instruction, consts: &[Value], casts: &[Cast]) -> Option<Operands> {
    use Kind::*;
    let (a, b, c) = (ins.a(), ins.b(), ins.c());
    let of = |reads: [Option<(u8, Kind)>; 3], writes: [Option<(u8, Kind)>; 2]| Operands {
        reads,
        writes,
        on: On::Both,
        moved: None,
        element: None,
        whatever: None,
    };
    let three = |kind: Kind, to: Kind| {
        of(
            [Some((b, kind)), Some((c, kind)), None],
            [Some((a, to)), None],
        )
    };
    let two = |kind: Kind, to: Kind| of([Some((b, kind)), None, None], [Some((a, to)), None]);
    let set = |kind: Kind| of([None; 3], [Some((a, kind)), None]);
    let test = |kind: Kind, with: Option<u8>| {
        of(
            [Some((a, kind)), with.map(|r| (r, kind)), None],
            [None; 2],
        )
    };
    // Registers `a` to `a + 3`; a frame has at most 255.
    let counted = |kind: Kind, on: On| -> Option<Operands> {
        let at = |i: u8| a.checked_add(i).map(|r| (r, kind));
        let var = Some(at(3)?);
        Some(Operands {
            reads: [at(0), at(1), at(2)],
            // Starting writes the variable; going round again writes the
            // counter too.
            writes: if on == On::Next {
                [var, None]
            } else {
                [at(0), var]
            },
            on,
            ..of([None; 3], [None; 2])
        })
    };
    Some(match ins.op()? {
        Op::MOVE => Operands {
            moved: Some((a, b)),
            ..of([None; 3], [None; 2])
        },
        Op::LOADI => set(Int),
        Op::LOADF => set(Float),
        Op::LOADBOOL => set(Bool),
        Op::LOADK => set(match consts.get(ins.bx() as usize)? {
            Value::Int(_) => Int,
            Value::Float(_) => Float,
            _ => return None,
        }),
        Op::ADDI | Op::SUBI | Op::MULI | Op::DIVI | Op::MODI => three(Int, Int),
        Op::ADDF | Op::SUBF | Op::MULF | Op::DIVF => three(Float, Float),
        Op::ADDII | Op::SUBII | Op::MULII | Op::NEGI => two(Int, Int),
        Op::NEGF => two(Float, Float),
        Op::CONV => match casts.get(c as usize)? {
            Cast::ToFloat => two(Int, Float),
            Cast::ToInt => two(Float, Int),
            Cast::Other => return None,
        },
        Op::LTI | Op::LEI | Op::EQI => three(Int, Bool),
        Op::LTF | Op::LEF | Op::EQF => three(Float, Bool),
        Op::NOT => two(Bool, Bool),
        // A jump that closes upvalues is the main loop's.
        Op::JMP if a == 0 => of([None; 3], [None; 2]),
        Op::JLTI | Op::JLEI | Op::JGEI | Op::JGTI | Op::JEQI | Op::JNEI => test(Int, Some(b)),
        Op::JLTII | Op::JLEII | Op::JGTII | Op::JGEII | Op::JEQII | Op::JNEII => test(Int, None),
        Op::JLTF | Op::JLEF | Op::JGTF | Op::JGEF => test(Float, Some(b)),
        Op::TEST => test(Bool, None),
        Op::FORPREP_I => counted(Int, On::Next)?,
        Op::FORLOOP_I => counted(Int, On::Named)?,
        Op::FORPREP_F => counted(Float, On::Next)?,
        Op::FORLOOP_F => counted(Float, On::Named)?,
        // With `!` or without: an element that is there is the same
        // either way, and one that is not is the main loop's.
        Op::GETIDXU | Op::GETIDX => Operands {
            element: Some(a),
            ..of([Some((b, Table)), Some((c, Int)), None], [None; 2])
        },
        Op::SETIDX => Operands {
            whatever: Some(c),
            ..of([Some((a, Table)), Some((b, Int)), None], [None; 2])
        },
        Op::LEN => of([Some((b, Table)), None, None], [Some((a, Int)), None]),
        _ => return None,
    })
}

/// What a `CONV` turns its operand into, of the conversions a plan runs:
/// between the two kinds of number. Both always succeed.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cast {
    /// An integer, as the float it is.
    ToFloat,
    /// A float, cut off towards zero.
    ToInt,
    Other,
}

impl Cast {
    pub(crate) fn of(ty: &saule_ast::Type) -> Cast {
        match ty {
            saule_ast::Type::Named(name) if name == "float" => Cast::ToFloat,
            saule_ast::Type::Named(name) if name == "integer" => Cast::ToInt,
            _ => Cast::Other,
        }
    }
}

/// How many turns a `for` from `from` to `limit` by `step` makes, for
/// [`Plan::worth`]; `step` is not 0.
#[inline]
pub(crate) fn turns(from: i64, limit: i64, step: i64) -> u64 {
    let turns = (limit as i128 - from as i128) / step as i128 + 1;
    turns.clamp(0, u64::MAX as i128) as u64
}

/// [`turns`], of a `for` over floats: a count that is not a number is
/// taken for a long one.
#[inline]
pub(crate) fn turns_float(from: f64, limit: f64, step: f64) -> u64 {
    let turns = ((limit - from) / step).floor() + 1.0;
    if turns.is_nan() { u64::MAX } else { turns as u64 }
}

impl Plan {
    /// A plan for the loop that `code[at]` starts or ends, or `None` when
    /// there is no such loop or it is not one a plan can hold: see the
    /// module docs for what it needs.
    ///
    /// `at` is the `FORPREP` of a numeric `for`, or a jump back.
    pub(crate) fn of(
        code: &[Instruction],
        consts: &[Value],
        casts: &[Cast],
        at: usize,
    ) -> Option<Plan> {
        let ins = *code.get(at)?;
        let (first, past) = match ins.op()? {
            // A loop that does not run jumps to just past its `FORLOOP`.
            Op::FORPREP_I | Op::FORPREP_F => (at + 1, jump(at + 1, ins.sbx())),
            // From where the jump lands to the jump itself.
            Op::JMP if ins.a() == 0 && ins.sbx() < 0 => (jump(at + 1, ins.sbx()), at + 1),
            _ => return None,
        };
        if past <= first || past > code.len() {
            return None;
        }
        Plan::over(code, consts, casts, first, past)
    }

    /// A plan for `code[first..past]`, entered at its first instruction.
    fn over(
        code: &[Instruction],
        consts: &[Value],
        casts: &[Cast],
        first: usize,
        past: usize,
    ) -> Option<Plan> {
        let body = &code[first..past];
        let n = body.len();

        // The steps, one for each instruction, and the one or two steps
        // each can go on to.
        let mut all = Vec::with_capacity(n);
        let mut steps = Vec::with_capacity(n + 2);
        let mut next: Vec<[Option<usize>; 2]> = Vec::with_capacity(n);
        // A jump lands on a step of the loop, or leaves it.
        let land = |target: usize| -> (bool, i64) {
            if (first..past).contains(&target) {
                (true, (target - first) as i64)
            } else {
                (false, target as i64)
            }
        };
        for (i, ins) in body.iter().enumerate() {
            all.push(operands(*ins, consts, casts)?);
            let (a, b, c) = (ins.a(), ins.b(), ins.c());
            let plain = |does: Does, n: i64| Step { does, a, b, c, n };
            let through = [Some(i + 1), None];
            let skips = [Some(i + 1), Some(i + 2)];
            let (step, goes) = match ins.op()? {
                Op::MOVE => (plain(Does::Move, 0), through),
                Op::LOADI => (
                    Step {
                        c: Kind::Int as u8,
                        ..plain(Does::Load, ins.sbx() as i64)
                    },
                    through,
                ),
                Op::LOADF => (
                    Step {
                        c: Kind::Float as u8,
                        ..plain(Does::Load, (ins.sbx() as f64).to_bits() as i64)
                    },
                    through,
                ),
                Op::LOADBOOL => (
                    Step {
                        c: Kind::Bool as u8,
                        ..plain(Does::Load, (b != 0) as i64)
                    },
                    through,
                ),
                Op::LOADK => {
                    let (kind, bits) = match consts.get(ins.bx() as usize)? {
                        Value::Int(v) => (Kind::Int, *v),
                        Value::Float(v) => (Kind::Float, v.to_bits() as i64),
                        _ => return None,
                    };
                    (
                        Step {
                            c: kind as u8,
                            ..plain(Does::Load, bits)
                        },
                        through,
                    )
                }
                Op::ADDI => (plain(Does::AddI, 0), through),
                Op::SUBI => (plain(Does::SubI, 0), through),
                Op::MULI => (plain(Does::MulI, 0), through),
                Op::DIVI => (plain(Does::DivI, 0), through),
                Op::MODI => (plain(Does::ModI, 0), through),
                Op::NEGI => (plain(Does::NegI, 0), through),
                Op::ADDII => (plain(Does::AddII, ins.sc()), through),
                Op::SUBII => (plain(Does::SubII, ins.sc()), through),
                Op::MULII => (plain(Does::MulII, ins.sc()), through),
                Op::ADDF => (plain(Does::AddF, 0), through),
                Op::SUBF => (plain(Does::SubF, 0), through),
                Op::MULF => (plain(Does::MulF, 0), through),
                Op::DIVF => (plain(Does::DivF, 0), through),
                Op::NEGF => (plain(Does::NegF, 0), through),
                Op::CONV => match casts.get(c as usize)? {
                    Cast::ToFloat => (plain(Does::ToFloat, 0), through),
                    Cast::ToInt => (plain(Does::ToInt, 0), through),
                    Cast::Other => return None,
                },
                Op::NOT => (plain(Does::Not, 0), through),
                Op::LTI => (plain(Does::LtI, 0), through),
                Op::LEI => (plain(Does::LeI, 0), through),
                Op::EQI => (plain(Does::EqI, 0), through),
                Op::LTF => (plain(Does::LtF, 0), through),
                Op::LEF => (plain(Does::LeF, 0), through),
                Op::EQF => (plain(Does::EqF, 0), through),
                Op::JMP => match land(jump(first + i + 1, ins.sbx())) {
                    (true, to) => (plain(Does::Jump, to), [None, Some(to as usize)]),
                    (false, to) => (plain(Does::Leave, to), [None, None]),
                },
                Op::JLTI => (plain(Does::JltI, 0), skips),
                Op::JLEI => (plain(Does::JleI, 0), skips),
                Op::JGEI => (plain(Does::JgeI, 0), skips),
                Op::JGTI => (plain(Does::JgtI, 0), skips),
                Op::JEQI => (plain(Does::JeqI, 0), skips),
                Op::JNEI => (plain(Does::JneI, 0), skips),
                Op::JLTII => (plain(Does::JltII, ins.sc()), skips),
                Op::JLEII => (plain(Does::JleII, ins.sc()), skips),
                Op::JGTII => (plain(Does::JgtII, ins.sc()), skips),
                Op::JGEII => (plain(Does::JgeII, ins.sc()), skips),
                Op::JEQII => (plain(Does::JeqII, ins.sc()), skips),
                Op::JNEII => (plain(Does::JneII, ins.sc()), skips),
                Op::JLTF => (plain(Does::JltF, 0), skips),
                Op::JLEF => (plain(Does::JleF, 0), skips),
                Op::JGTF => (plain(Does::JgtF, 0), skips),
                Op::JGEF => (plain(Does::JgeF, 0), skips),
                Op::TEST => (plain(Does::Test, 0), skips),
                op @ (Op::FORPREP_I | Op::FORPREP_F | Op::FORLOOP_I | Op::FORLOOP_F) => {
                    // Both go on to the next step, or to where they say:
                    // a step of this plan, or the instruction after it.
                    let (inside, to) = land(jump(first + i + 1, ins.sbx()));
                    if !inside && to as usize != past {
                        return None;
                    }
                    let to = if inside { to } else { n as i64 };
                    let does = match op {
                        Op::FORPREP_I => Does::PrepI,
                        Op::FORPREP_F => Does::PrepF,
                        Op::FORLOOP_I => Does::LoopI,
                        _ => Does::LoopF,
                    };
                    (plain(does, to), [Some(i + 1), Some(to as usize)])
                }
                // Which of the four it is waits on what the result is read
                // as, below.
                Op::GETIDXU | Op::GETIDX => (plain(Does::GetI, 0), through),
                Op::SETIDX => (plain(Does::Put, 0), through),
                Op::LEN => (plain(Does::Len, 0), through),
                _ => return None,
            };
            steps.push(step);
            next.push(goes);
        }
        // Past the last instruction: on with the one after the loop. A
        // test that skips the last instruction lands one further on, and
        // leaves there too, for the instruction it skipped to.
        for after in [past, past + 1] {
            steps.push(Step {
                does: Does::Leave,
                a: 0,
                b: 0,
                c: 0,
                n: after as i64,
            });
        }

        // What each register can hold on the way into each step: a bit
        // for each kind the loop may have written there, and one for what
        // it held when the loop started. Followed forwards from the first
        // step, along every way on, until nothing is added.
        //
        // Two things are not known the first time round: what kind a
        // register was when the loop started (`taken`), which a `MOVE`
        // hands on, and what kind an element read out of a table is
        // (`element`). Both are what the reads take them for, so the
        // whole is done again with what the reads have said until they
        // say nothing new.
        let mut taken: [Option<Kind>; 256] = [None; 256];
        let mut element: [Option<Kind>; 256] = [None; 256];
        let mut can;
        let mut reached;
        loop {
            can = vec![[0u8; 256]; n + 2];
            can[0] = [AS_IT_WAS; 256];
            reached = vec![false; n + 2];
            reached[0] = true;
            loop {
                let mut changed = false;
                for i in 0..n {
                    if !reached[i] {
                        continue;
                    }
                    let ops = &all[i];
                    for (way, to) in next[i].iter().enumerate() {
                        let Some(to) = *to else {
                            continue;
                        };
                        let mut out = can[i];
                        let writes = match ops.on {
                            On::Both => true,
                            On::Next => way == 0,
                            On::Named => way == 1,
                        };
                        if writes {
                            for (r, kind) in ops.writes.iter().flatten() {
                                out[*r as usize] = kind.bit();
                            }
                            if let Some((to, from)) = ops.moved {
                                let holds = can[i][from as usize];
                                out[to as usize] =
                                    match (holds & AS_IT_WAS != 0, taken[from as usize]) {
                                        (false, _) => holds,
                                        (true, Some(kind)) => holds & !AS_IT_WAS | kind.bit(),
                                        (true, None) => holds & !AS_IT_WAS | UNKNOWN,
                                    };
                            }
                            if let Some(r) = ops.element {
                                out[r as usize] = match element[r as usize] {
                                    Some(kind) => kind.bit(),
                                    None => ELEMENT,
                                };
                            }
                        }
                        if !reached[to] {
                            reached[to] = true;
                            changed = true;
                        }
                        for (into, from) in can[to].iter_mut().zip(out) {
                            if *into | from != *into {
                                *into |= from;
                                changed = true;
                            }
                        }
                    }
                }
                if !changed {
                    break;
                }
            }

            // What the reads say: of a register they may find as it was,
            // that kind then; of one that may hold an element, that kind
            // of element. The same for every such read.
            let before = (taken, element);
            for (i, ops) in all.iter().enumerate() {
                if !reached[i] {
                    continue;
                }
                for (r, kind) in ops.reads.iter().flatten() {
                    let holds = can[i][*r as usize];
                    for (flag, said) in [(AS_IT_WAS, &mut taken), (ELEMENT, &mut element)] {
                        if holds & flag == 0 {
                            continue;
                        }
                        match said[*r as usize] {
                            None => said[*r as usize] = Some(*kind),
                            Some(was) if was == *kind => {}
                            Some(_) => return None,
                        }
                    }
                }
            }
            // A `MOVE` out of a register that may be as it was, of which
            // no read has said anything: it is taken for what its copy is
            // read as, anywhere in the loop. Only ever a demand made of
            // the register when the loop starts, so a wrong guess costs
            // the plan (the reads below then do not agree) and nothing
            // else.
            for (i, ops) in all.iter().enumerate() {
                let Some((to, from)) = ops.moved else {
                    continue;
                };
                if !reached[i]
                    || can[i][from as usize] & AS_IT_WAS == 0
                    || taken[from as usize].is_some()
                {
                    continue;
                }
                taken[from as usize] = all
                    .iter()
                    .flat_map(|ops| ops.reads.iter().flatten())
                    .find(|(r, _)| *r == to)
                    .map(|(_, kind)| *kind);
            }
            if (taken, element) == before {
                break;
            }
        }

        // Every read has to find its kind and no other, every `MOVE` a
        // source whose kind is known, and every element read a kind to
        // look for.
        let mut written = [false; 256];
        let mut stored = [false; 256];
        let mut touched = [false; 256];
        for (i, ops) in all.iter().enumerate() {
            if !reached[i] {
                continue;
            }
            for (r, kind) in ops.reads.iter().flatten() {
                if can[i][*r as usize] & !(AS_IT_WAS | kind.bit()) != 0 {
                    return None;
                }
                touched[*r as usize] = true;
            }
            for (r, _) in ops.writes.iter().flatten() {
                written[*r as usize] = true;
                touched[*r as usize] = true;
            }
            if let Some((to, from)) = ops.moved {
                let holds = can[i][from as usize];
                if holds & (UNKNOWN | ELEMENT) != 0
                    || (holds & AS_IT_WAS != 0 && taken[from as usize].is_none())
                {
                    return None;
                }
                written[to as usize] = true;
                touched[to as usize] = true;
                touched[from as usize] = true;
            }
            if let Some(r) = ops.element {
                steps[i].does = match element[r as usize]? {
                    Kind::Int => Does::GetI,
                    Kind::Float => Does::GetF,
                    Kind::Bool => Does::GetB,
                    Kind::Table => Does::GetT,
                };
                written[r as usize] = true;
                touched[r as usize] = true;
            }
            if let Some(r) = ops.whatever {
                let holds = can[i][r as usize];
                if holds & (UNKNOWN | ELEMENT) != 0 {
                    return None;
                }
                // As it was, and nothing says what that is: taken if it
                // is a number, when the loop starts.
                if holds & AS_IT_WAS != 0 && taken[r as usize].is_none() {
                    stored[r as usize] = true;
                }
                touched[r as usize] = true;
            }
        }

        // The registers by their place in the plan's arrays. The four of a
        // `for` are next to each other in the frame and are all touched,
        // so they are next to each other here.
        let used: Vec<u8> = (0..=255u8).filter(|r| touched[*r as usize]).collect();
        if used.len() > MOST {
            return None;
        }
        let mut place = [0u8; 256];
        for (i, r) in used.iter().enumerate() {
            place[*r as usize] = i as u8;
        }
        for step in steps.iter_mut().take(n) {
            let [a, b, c] = step.does.registers();
            if matches!(
                step.does,
                Does::PrepI | Does::LoopI | Does::PrepF | Does::LoopF
            ) {
                let at = place[step.a as usize];
                for i in 1..=3u8 {
                    let r = step.a.checked_add(i)?;
                    if !touched[r as usize] || place[r as usize] != at + i {
                        return None;
                    }
                }
            }
            if a {
                step.a = place[step.a as usize];
            }
            if b {
                step.b = place[step.b as usize];
            }
            if c {
                step.c = place[step.c as usize];
            }
        }
        let placed = |which: &dyn Fn(u8) -> bool| -> Vec<u8> {
            used.iter()
                .filter(|r| which(**r))
                .map(|r| place[*r as usize])
                .collect()
        };
        Some(Plan {
            first,
            taken: used
                .iter()
                .filter_map(|r| taken[*r as usize].map(|kind| (place[*r as usize], kind)))
                .collect(),
            stored: placed(&|r| stored[r as usize] && taken[r as usize].is_none()),
            written: placed(&|r| written[r as usize]),
            steps,
            used,
            came: Cell::new(0),
        })
    }

    /// How many steps one turn of the loop is, at most.
    pub(crate) fn weight(&self) -> usize {
        self.steps.len() - 2
    }

    /// Whether a `for` of `turns` turns is worth handing over.
    ///
    /// Handing over costs what clearing the plan's arrays and copying its
    /// registers in and out costs: some tens of steps' worth, which a
    /// loop of three turns inside a longer one would pay every time round
    /// to save less.
    #[inline]
    pub(crate) fn worth(&self, turns: u64) -> bool {
        turns.saturating_mul(self.weight() as u64) >= 96
    }

    /// Whether a loop the main loop has just come to the jump back of is
    /// to be handed over now.
    ///
    /// A `while` does not say how long it will go on. So the main loop
    /// runs its first turns, and it is handed over at every eighth time
    /// the jump is come to: a loop of a hundred turns runs ninety of them
    /// here, and one of three turns is hardly ever asked.
    #[inline]
    pub(crate) fn due(&self) -> bool {
        let came = self.came.get() + 1;
        if came < 8 {
            self.came.set(came);
            return false;
        }
        self.came.set(0);
        true
    }

    /// Run the loop on `regs`, the frame's registers, and return the place
    /// of the instruction to go on with: the one after the loop, or one
    /// inside it that is the main loop's to execute. `None`, with nothing
    /// done, when a register the loop reads does not hold what the loop
    /// takes it for: the loop is then the main loop's from its start.
    ///
    /// # Safety
    ///
    /// `regs` is `R[0]` of the frame running the proto this plan was made
    /// from, whose verifier kept every operand inside the frame's
    /// registers (`compile::verify`).
    #[inline(never)]
    pub(crate) unsafe fn run(&self, regs: *mut Value) -> Option<usize> {
        // The numbers, by place, and the kind each was last written as; 0
        // for a register not read from the frame and not written yet,
        // which is then never read (`Plan::over` saw to that) and is left
        // as it is when the loop leaves. A table is held beside its
        // number, which is nothing.
        let mut words = [0u64; MOST];
        let mut kinds = [0u8; MOST];
        let mut held: [Option<Table>; MOST] = [const { None }; MOST];
        // SAFETY, for every `reg`: a register of the frame (see above).
        let reg = |place: u8| unsafe { regs.add(self.used[place as usize] as usize) };
        for &(place, kind) in &self.taken {
            let bits = match (unsafe { &*reg(place) }, kind) {
                (Value::Int(v), Kind::Int) => *v as u64,
                (Value::Float(v), Kind::Float) => v.to_bits(),
                (Value::Bool(v), Kind::Bool) => *v as u64,
                (Value::Table(t), Kind::Table) => {
                    held[place as usize] = Some(Rc::clone(t));
                    0
                }
                _ => return None,
            };
            words[place as usize] = bits;
            kinds[place as usize] = kind as u8;
        }
        for &place in &self.stored {
            let (bits, kind) = match unsafe { &*reg(place) } {
                Value::Int(v) => (*v as u64, Kind::Int),
                Value::Float(v) => (v.to_bits(), Kind::Float),
                Value::Bool(v) => (*v as u64, Kind::Bool),
                _ => continue,
            };
            words[place as usize] = bits;
            kinds[place as usize] = kind as u8;
        }

        let at = self.steps(&mut words, &mut kinds, &mut held);

        for &place in &self.written {
            let bits = words[place as usize];
            let value = match kinds[place as usize] {
                0 => continue,
                1 => Value::Int(bits as i64),
                2 => Value::Float(f64::from_bits(bits)),
                3 => Value::Bool(bits != 0),
                _ => match held[place as usize].take() {
                    Some(table) => Value::Table(table),
                    None => continue,
                },
            };
            unsafe { put(reg(place), value) };
        }
        Some(at)
    }

    /// The loop itself. Returns the place of the instruction to go on
    /// with. A function of its own, so that it has nothing in it but the
    /// loop.
    #[inline(never)]
    fn steps(
        &self,
        words: &mut [u64; MOST],
        kinds: &mut [u8; MOST],
        held: &mut [Option<Table>; MOST],
    ) -> usize {
        // SAFETY, for every index into the three arrays below: a step
        // names a register by its place, which `Plan::over` made less than
        // the number of registers the plan uses, and that no more than
        // `MOST`; the four of a `for` are `a` to `a + 3`, which it checked
        // are four places in a row.
        macro_rules! word {
            ($i:expr) => {
                *unsafe { words.get_unchecked_mut($i as usize) }
            };
        }
        macro_rules! kind {
            ($i:expr) => {
                *unsafe { kinds.get_unchecked_mut($i as usize) }
            };
        }
        macro_rules! table {
            ($i:expr) => {
                *unsafe { held.get_unchecked_mut($i as usize) }
            };
        }
        /// `R[i]` as an integer, a float.
        macro_rules! int {
            ($i:expr) => {
                word!($i) as i64
            };
        }
        macro_rules! float {
            ($i:expr) => {
                f64::from_bits(word!($i))
            };
        }
        /// `R[i] = bits`, of kind `k`.
        macro_rules! set {
            ($i:expr, $bits:expr, $k:expr) => {{
                let i = $i as usize;
                word!(i) = $bits;
                kind!(i) = $k;
            }};
        }
        macro_rules! set_int {
            ($i:expr, $v:expr) => {
                set!($i, $v as u64, Kind::Int as u8)
            };
        }
        macro_rules! set_float {
            ($i:expr, $v:expr) => {
                set!($i, f64::to_bits($v), Kind::Float as u8)
            };
        }
        macro_rules! set_bool {
            ($i:expr, $v:expr) => {
                set!($i, $v as u64, Kind::Bool as u8)
            };
        }
        /// The element `R[b][R[c]]`, as the main loop finds it when the
        /// index is inside the array, or back to the main loop: with the
        /// instruction, for an index that is not, a table lent out, or no
        /// table at all.
        macro_rules! element {
            ($s:expr, $at:expr, |$v:ident| $then:expr) => {{
                let Some(table) = table!($s.b).as_deref() else {
                    return self.first + $at;
                };
                let Ok(table) = table.try_borrow() else {
                    return self.first + $at;
                };
                let Some(slot) = table.array_index(int!($s.c)) else {
                    return self.first + $at;
                };
                // SAFETY: `array_index` answers with a slot of the array.
                let $v = unsafe { table.array.get_unchecked(slot) };
                $then
            }};
        }

        let steps = self.steps.as_slice();
        let mut at = 0usize;
        /// One step: fetched, executed, and `at` moved on. Written once and
        /// placed four times in the loop below, so that there are four
        /// places a step is jumped to from and not one: each sees every
        /// fourth step, which in a short loop is a pattern the processor
        /// follows, where one place sees them all in turn. Measured on
        /// `loop_arith`, whose loop is four steps: 110M cycles with one,
        /// 103M with two, 96M with four; `mandel` and `closure` the same
        /// or a little better.
        macro_rules! step {
            () => {{
                // SAFETY: every step is followed by another until a `Leave`,
                // which returns: `Plan::over` ends the steps with two, a jump
                // goes to a step of the loop, and a test skips one step.
                let s = unsafe { *steps.get_unchecked(at) };
                match s.does {
                    Does::Move => {
                        word!(s.a) = word!(s.b);
                        kind!(s.a) = kind!(s.b);
                        if kind!(s.b) == Kind::Table as u8 {
                            table!(s.a) = table!(s.b).clone();
                        }
                    }
                    Does::Load => set!(s.a, s.n as u64, s.c),
                    Does::AddI => set_int!(s.a, int!(s.b).wrapping_add(int!(s.c))),
                    Does::SubI => set_int!(s.a, int!(s.b).wrapping_sub(int!(s.c))),
                    Does::MulI => set_int!(s.a, int!(s.b).wrapping_mul(int!(s.c))),
                    Does::DivI => {
                        let by = int!(s.c);
                        if by == 0 {
                            return self.first + at;
                        }
                        set_int!(s.a, int!(s.b).wrapping_div(by));
                    }
                    Does::ModI => {
                        let by = int!(s.c);
                        if by == 0 {
                            return self.first + at;
                        }
                        set_int!(s.a, int!(s.b).wrapping_rem(by));
                    }
                    Does::NegI => set_int!(s.a, int!(s.b).wrapping_neg()),
                    Does::AddII => set_int!(s.a, int!(s.b).wrapping_add(s.n)),
                    Does::SubII => set_int!(s.a, int!(s.b).wrapping_sub(s.n)),
                    Does::MulII => set_int!(s.a, int!(s.b).wrapping_mul(s.n)),
                    Does::AddF => set_float!(s.a, float!(s.b) + float!(s.c)),
                    Does::SubF => set_float!(s.a, float!(s.b) - float!(s.c)),
                    Does::MulF => set_float!(s.a, float!(s.b) * float!(s.c)),
                    Does::DivF => set_float!(s.a, float!(s.b) / float!(s.c)),
                    Does::NegF => set_float!(s.a, -float!(s.b)),
                    Does::ToFloat => set_float!(s.a, int!(s.b) as f64),
                    Does::ToInt => set_int!(s.a, float!(s.b).trunc() as i64),
                    Does::Not => set_bool!(s.a, word!(s.b) == 0),
                    Does::LtI => set_bool!(s.a, int!(s.b) < int!(s.c)),
                    Does::LeI => set_bool!(s.a, int!(s.b) <= int!(s.c)),
                    Does::EqI => set_bool!(s.a, int!(s.b) == int!(s.c)),
                    Does::LtF => set_bool!(s.a, float!(s.b) < float!(s.c)),
                    Does::LeF => set_bool!(s.a, float!(s.b) <= float!(s.c)),
                    Does::EqF => set_bool!(s.a, float!(s.b) == float!(s.c)),
                    Does::Jump => {
                        at = s.n as usize;
                        continue;
                    }
                    Does::Leave => return s.n as usize,
                    Does::JltI => at += (int!(s.a) < int!(s.b)) as usize,
                    Does::JleI => at += (int!(s.a) <= int!(s.b)) as usize,
                    Does::JgeI => at += (int!(s.a) >= int!(s.b)) as usize,
                    Does::JgtI => at += (int!(s.a) > int!(s.b)) as usize,
                    Does::JeqI => at += (int!(s.a) == int!(s.b)) as usize,
                    Does::JneI => at += (int!(s.a) != int!(s.b)) as usize,
                    Does::JltII => at += (int!(s.a) < s.n) as usize,
                    Does::JleII => at += (int!(s.a) <= s.n) as usize,
                    Does::JgtII => at += (int!(s.a) > s.n) as usize,
                    Does::JgeII => at += (int!(s.a) >= s.n) as usize,
                    Does::JeqII => at += (int!(s.a) == s.n) as usize,
                    Does::JneII => at += (int!(s.a) != s.n) as usize,
                    Does::JltF => at += (float!(s.a) < float!(s.b)) as usize,
                    Does::JleF => at += (float!(s.a) <= float!(s.b)) as usize,
                    Does::JgtF => at += (float!(s.a) > float!(s.b)) as usize,
                    Does::JgeF => at += (float!(s.a) >= float!(s.b)) as usize,
                    Does::Test => {
                        at += ((word!(s.a) != 0) != (s.c != 0)) as usize
                    }
                    Does::PrepI => {
                        let (from, limit, step) = (int!(s.a), int!(s.a + 1), int!(s.a + 2));
                        if step == 0 {
                            return self.first + at;
                        }
                        if int_in_range(from, limit, step) {
                            set_int!(s.a + 3, from);
                        } else {
                            at = s.n as usize;
                            continue;
                        }
                    }
                    Does::LoopI => {
                        let (i, limit, step) = (int!(s.a), int!(s.a + 1), int!(s.a + 2));
                        let (next, overflow) = i.overflowing_add(step);
                        if !overflow && int_in_range(next, limit, step) {
                            set_int!(s.a, next);
                            set_int!(s.a + 3, next);
                            at = s.n as usize;
                            continue;
                        }
                    }
                    Does::PrepF => {
                        let (from, limit, step) =
                            (float!(s.a), float!(s.a + 1), float!(s.a + 2));
                        if step == 0.0 {
                            return self.first + at;
                        }
                        if float_in_range(from, limit, step) {
                            set_float!(s.a + 3, from);
                        } else {
                            at = s.n as usize;
                            continue;
                        }
                    }
                    Does::LoopF => {
                        let (i, limit, step) = (float!(s.a), float!(s.a + 1), float!(s.a + 2));
                        let next = i + step;
                        if float_in_range(next, limit, step) {
                            set_float!(s.a, next);
                            set_float!(s.a + 3, next);
                            at = s.n as usize;
                            continue;
                        }
                    }
                    Does::GetI => {
                        let v = element!(s, at, |v| match v {
                            Value::Int(v) => *v,
                            _ => return self.first + at,
                        });
                        set_int!(s.a, v);
                    }
                    Does::GetF => {
                        let v = element!(s, at, |v| match v {
                            Value::Float(v) => *v,
                            _ => return self.first + at,
                        });
                        set_float!(s.a, v);
                    }
                    Does::GetB => {
                        let v = element!(s, at, |v| match v {
                            Value::Bool(v) => *v,
                            _ => return self.first + at,
                        });
                        set_bool!(s.a, v);
                    }
                    Does::GetT => {
                        let v = element!(s, at, |v| match v {
                            Value::Table(v) => Rc::clone(v),
                            _ => return self.first + at,
                        });
                        table!(s.a) = Some(v);
                        set!(s.a, 0, Kind::Table as u8);
                    }
                    Does::Put => {
                        let bits = word!(s.c);
                        let v = match kind!(s.c) {
                            1 => Value::Int(bits as i64),
                            2 => Value::Float(f64::from_bits(bits)),
                            3 => Value::Bool(bits != 0),
                            // A table going into a table is the collector's
                            // to hear of, and a register that was nothing
                            // the loop took in is the main loop's to read.
                            _ => return self.first + at,
                        };
                        let Some(table) = table!(s.a).as_deref() else {
                            return self.first + at;
                        };
                        let Ok(mut table) = table.try_borrow_mut() else {
                            return self.first + at;
                        };
                        let Some(slot) = table.array_index(int!(s.b)) else {
                            return self.first + at;
                        };
                        // Through `DerefMut`, as every write to an array
                        // goes: a `for … in` over it gets its copy first.
                        table.array[slot].store(v);
                    }
                    Does::Len => {
                        let Some(table) = table!(s.b).as_deref() else {
                            return self.first + at;
                        };
                        let Ok(table) = table.try_borrow() else {
                            return self.first + at;
                        };
                        let n = table.array_len() as i64;
                        drop(table);
                        set_int!(s.a, n);
                    }
                }
                at += 1;
            }};
        }
        loop {
            step!();
            step!();
            step!();
            step!();
        }
    }
}

/// The plans of one proto: one for each loop in it that can have one.
/// Made when the first of its loops starts.
pub(crate) struct Plans {
    /// A bit for each instruction, set at the `FORPREP` or the jump back
    /// of a loop with a plan: what a loop without one pays to be asked.
    marked: Vec<u64>,
    /// The plans, by the place of that instruction.
    made: Vec<(u32, Plan)>,
}

impl Plans {
    /// The plans for `code`, a proto of `chunk` called `name`.
    ///
    /// `SAULE_PLAIN=0` makes none, so that every loop is the main loop's:
    /// what a program does must not depend on which ran it, and running
    /// it both ways is how that is checked. `SAULE_PLAIN=trace` says on
    /// standard error which loops got a plan.
    pub(crate) fn of(name: &str, code: &[Instruction], chunk: &crate::chunk::Chunk) -> Plans {
        let mut plans = Plans {
            marked: vec![0; code.len().div_ceil(64)],
            made: Vec::new(),
        };
        let asked = std::env::var("SAULE_PLAIN").unwrap_or_default();
        if asked == "0" {
            return plans;
        }
        let casts: Vec<Cast> = chunk.cast_types.iter().map(|ty| Cast::of(ty)).collect();
        let consts = &chunk.constants;
        for at in 0..code.len() {
            if let Some(plan) = Plan::of(code, consts, &casts, at) {
                if asked == "trace" {
                    eprintln!(
                        "plain: the loop at {at} of {name}, {} instructions",
                        plan.weight()
                    );
                }
                plans.marked[at / 64] |= 1 << (at % 64);
                plans.made.push((at as u32, plan));
            }
        }
        plans
    }

    /// The plan for the loop whose `FORPREP` or jump back is at `pc`, if
    /// it has one.
    #[inline(always)]
    pub(crate) fn at(&self, pc: u32) -> Option<&Plan> {
        let word = self.marked.get(pc as usize / 64)?;
        if word >> (pc % 64) & 1 == 0 {
            return None;
        }
        self.find(pc)
    }

    #[inline(never)]
    fn find(&self, pc: u32) -> Option<&Plan> {
        self.made
            .iter()
            .find(|(at, _)| *at == pc)
            .map(|(_, plan)| plan)
    }
}

impl std::fmt::Debug for Plans {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Plans({})", self.made.len())
    }
}
