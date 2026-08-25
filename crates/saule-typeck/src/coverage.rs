//! How much of a module the type table actually covers.
//!
//! §24.1 of `VM_DESIGN.md` names this as the first risk to the whole VM
//! project: if `infer` answers `None` more often than assumed, most
//! arithmetic degrades to the dynamic `ARITHX` form and the projected
//! speed-up collapses. The mitigation it prescribes is to *measure before
//! depending on it*, which is what this module is for.
//!
//! Two numbers matter, and they are not the same number:
//!
//! * **Overall coverage** — how many expression nodes carry a type. Useful
//!   as a trend line.
//! * **Arithmetic-operand coverage** — how many operands of `+ - * / % ^`
//!   and the bitwise operators carry a type. This is the one §24.1 sets a
//!   bar for (~90%), because it is what decides between `ADDI` and `ARITHX`.
//!
//! A node counted as covered is not necessarily *useful*: a recorded `any`
//! tells the compiler nothing it did not already assume. So arithmetic
//! operands are also counted as "numeric", meaning the recorded type is
//! concretely `integer` or `float` — the answer that actually selects a
//! typed opcode.

use saule_ast::{BinOp, Expr, Module, Type};

use crate::table::TypeTable;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Coverage {
    /// Every expression node in the module.
    pub exprs: usize,
    /// Expression nodes with a recorded type.
    pub exprs_typed: usize,
    /// Operands of an arithmetic or bitwise binary operator.
    pub arith_operands: usize,
    /// Of those, how many carry any recorded type.
    pub arith_typed: usize,
    /// Of those, how many are concretely `integer` or `float` — the answer
    /// that selects a typed opcode rather than the dynamic fallback.
    pub arith_numeric: usize,
}

impl Coverage {
    pub fn expr_percent(&self) -> f64 {
        percent(self.exprs_typed, self.exprs)
    }

    pub fn arith_percent(&self) -> f64 {
        percent(self.arith_typed, self.arith_operands)
    }

    /// The figure §24.1's ~90% bar applies to.
    pub fn arith_numeric_percent(&self) -> f64 {
        percent(self.arith_numeric, self.arith_operands)
    }

    /// Sum two reports, so a caller can total a whole directory.
    pub fn merge(&mut self, other: &Coverage) {
        self.exprs += other.exprs;
        self.exprs_typed += other.exprs_typed;
        self.arith_operands += other.arith_operands;
        self.arith_typed += other.arith_typed;
        self.arith_numeric += other.arith_numeric;
    }

    /// One-line summary for `--dump-type-coverage`.
    pub fn summary(&self) -> String {
        format!(
            "expressions {}/{} ({:.1}%)  arithmetic operands {}/{} typed ({:.1}%), \
             {} numeric ({:.1}%)",
            self.exprs_typed,
            self.exprs,
            self.expr_percent(),
            self.arith_typed,
            self.arith_operands,
            self.arith_percent(),
            self.arith_numeric,
            self.arith_numeric_percent(),
        )
    }
}

fn percent(n: usize, d: usize) -> f64 {
    if d == 0 {
        100.0
    } else {
        n as f64 * 100.0 / d as f64
    }
}

/// Measure `table`'s coverage of `module`.
pub fn measure(module: &Module, table: &TypeTable) -> Coverage {
    let mut c = Coverage::default();
    saule_ast::visit_exprs(module, &mut |e| {
        c.exprs += 1;
        if table.contains_key(&e.id) {
            c.exprs_typed += 1;
        }
        // Count the *operands*, at the parent, so each is attributed to the
        // operator that will have to pick an opcode for it.
        if let Expr::Binary { op, lhs, rhs } = &e.value
            && selects_a_typed_opcode(*op)
        {
            for operand in [lhs, rhs] {
                c.arith_operands += 1;
                if let Some(t) = table.get(&operand.id) {
                    c.arith_typed += 1;
                    if is_numeric(t) {
                        c.arith_numeric += 1;
                    }
                }
            }
        }
    });
    c
}

/// Operators with an integer and a float opcode, whose selection depends on
/// the operand types. Comparisons are excluded: they produce a boolean
/// whatever the operands are, and `and`/`or`/`..`/`??` do not branch on
/// numeric type either.
fn selects_a_typed_opcode(op: BinOp) -> bool {
    matches!(
        op,
        BinOp::Add
            | BinOp::Sub
            | BinOp::Mul
            | BinOp::Div
            | BinOp::Mod
            | BinOp::Pow
            | BinOp::BAnd
            | BinOp::BOr
            | BinOp::BXor
            | BinOp::Shl
            | BinOp::Shr
    )
}

fn is_numeric(t: &Type) -> bool {
    matches!(t, Type::Named(n) if n == "integer" || n == "float")
}
