//! Disassembler (`VM_DESIGN.md` §17.1).
//!
//! Written before the compiler, on purpose: debugging a bytecode compiler
//! without one is miserable. Everything here is driven by [`Op::fmt`], so an
//! opcode added to the table prints correctly with no change to this file.

use std::fmt::Write as _;

use crate::chunk::{Chunk, Proto};
use crate::op::{Fmt, Instruction, Op};

/// Disassemble a whole chunk: every proto, its constants, and its classes.
pub fn chunk(c: &Chunk) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "; chunk from {}", c.source.name());
    let _ = writeln!(out, "; {} proto(s), {} constant(s), {} module slot(s)",
        c.protos.len(), c.constants.len(), c.module_slots);

    if !c.constants.is_empty() {
        let _ = writeln!(out, "\nconstants:");
        for (i, k) in c.constants.iter().enumerate() {
            let _ = writeln!(out, "  K[{i}] = {}", quote(k));
        }
    }

    for (i, cls) in c.classes.iter().enumerate() {
        let _ = writeln!(
            out,
            "\nclass[{i}] {} ({} field(s), {} vtable slot(s))",
            cls.name,
            cls.layout.len(),
            cls.vtable.len()
        );
    }

    for (i, p) in c.protos.iter().enumerate() {
        let _ = write!(out, "\n{}", proto(c, p, i as u32));
    }
    out
}

/// Disassemble one proto.
pub fn proto(c: &Chunk, p: &Proto, idx: u32) -> String {
    let mut out = String::new();
    let main = if idx == c.main { "  ; main" } else { "" };
    let _ = writeln!(
        out,
        "proto[{idx}] {}({} param{}{}) regs={} upvals={}{main}",
        p.label(),
        p.n_params,
        if p.n_params == 1 { "" } else { "s" },
        if p.is_variadic { ", …" } else { "" },
        p.max_regs,
        p.upvals.len()
    );

    for (i, u) in p.upvals.iter().enumerate() {
        let _ = writeln!(
            out,
            "  ; upval[{i}] {} <- parent {} {}",
            u.name,
            if u.from_parent_stack { "register" } else { "upvalue" },
            u.index
        );
    }
    for h in &p.handlers {
        let _ = writeln!(
            out,
            "  ; handler [{:04}..{:04}) -> {:04} into r{}",
            h.pc_start, h.pc_end, h.target, h.err_reg
        );
    }

    let mut pc = 0usize;
    while pc < p.code.len() {
        let ins = p.code[pc];
        let _ = writeln!(out, "  {:04}  {}", pc, instruction(c, p, ins, pc));
        pc += 1;
        // An EXTRAARG belongs to the instruction above it; print it indented
        // so the listing reads as one operation.
        if pc < p.code.len() && p.code[pc].op() == Some(Op::EXTRAARG) {
            let _ = writeln!(out, "  {:04}    | extra {}", pc, p.code[pc].ax());
            pc += 1;
        }
    }
    out
}

/// Format a single instruction. `pc` is the instruction's own index, used to
/// resolve jump displacements to absolute targets.
pub fn instruction(c: &Chunk, p: &Proto, ins: Instruction, pc: usize) -> String {
    let Some(op) = ins.op() else {
        return format!("<bad opcode {:#04x}>  ; word {:#010x}", ins.raw_op(), ins.0);
    };

    let operands = match op.fmt() {
        Fmt::Abc => format!("{:<5} {:<4} {:<4}", ins.a(), ins.b(), ins.c()),
        Fmt::ABx => format!("{:<5} {:<9}", ins.a(), ins.bx()),
        Fmt::AsBx => format!("{:<5} {:<9}", ins.a(), ins.sbx()),
        Fmt::Ax => format!("{:<15}", ins.ax()),
    };

    let mut line = format!("{:<10} {operands}", op.name());
    if let Some(note) = annotate(c, p, op, ins, pc) {
        let _ = write!(line, "  ; {note}");
    }
    line
}

/// The half of the listing a human actually reads: constants spelled out,
/// jumps resolved to absolute targets, callee names filled in.
fn annotate(c: &Chunk, p: &Proto, op: Op, ins: Instruction, pc: usize) -> Option<String> {
    match op {
        Op::LOADK | Op::JEQK => c.constants.get(ins.bx() as usize).map(quote),
        Op::GETMAPK => c.constants.get(ins.c() as usize).map(quote),
        Op::SETMAPK => c.constants.get(ins.b() as usize).map(quote),
        Op::CLOSURE => {
            let idx = *p.protos.get(ins.bx() as usize)?;
            let target = c.protos.get(idx as usize)?;
            Some(format!("proto[{idx}] {}", target.label()))
        }
        Op::NEW => c.classes.get(ins.bx() as usize).map(|cl| cl.name.to_string()),
        Op::VARIANT => c.enums.get(ins.bx() as usize).map(|e| e.name.to_string()),
        Op::SWITCH => {
            let t = c.jump_tables.get(ins.bx() as usize)?;
            Some(format!("{} arm(s), default -> {:04}", t.targets.len(), t.default))
        }
        // Resolving a displacement to an absolute target is the difference
        // between a readable listing and an unreadable one — but only for
        // opcodes whose sBx *is* a displacement. `LOADI` shares the layout
        // and carries a literal.
        _ if op.is_jump() => Some(format!("-> {:04}", (pc as i64 + 1 + ins.sbx() as i64).max(0))),
        Op::ITERPREP | Op::ITERPREPX => {
            Some(format!("-> {:04}", pc + 1 + ins.bx() as usize))
        }
        _ => None,
    }
}

fn quote(v: &saule_interpreter::Value) -> String {
    match v {
        saule_interpreter::Value::Str(s) => format!("{:?}", s.as_str()),
        other => other.to_display_string(),
    }
}
