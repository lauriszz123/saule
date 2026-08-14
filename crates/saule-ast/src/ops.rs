//! Operator-overloading contracts — the built-in `Op*` interfaces that let
//! a class define what `+`, `..`, `<`, `#`, … mean for its instances.
//!
//! This is the single source of truth shared by every stage:
//!
//! * `saule-semantic` pre-registers the interface names so
//!   `implements OpAdd<…>` resolves without an import;
//! * `saule-typeck` uses the contracts to decide whether an operator is
//!   legal on a class and what type the result has;
//! * `saule-interpreter` installs the interface values and dispatches the
//!   operator to the contract method at runtime.
//!
//! Saule's answer to Lua's `__add` / `__sub` / `__concat` / … metamethods,
//! with one interface per operator so a class opts into exactly the
//! operators it can support.

use crate::{BinOp, UnaryOp};

/// One operator's overloading contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperatorContract {
    /// Built-in interface a class lists in its `implements` clause.
    pub interface: &'static str,
    /// Method the interface requires, and the one dispatch calls.
    pub method: &'static str,
    /// Parameters the method takes — 1 for binary operators, 0 for the
    /// unary ones and `toString`.
    pub params: usize,
    /// The operator as written in source, for diagnostics.
    pub symbol: &'static str,
}

macro_rules! contract {
    ($iface:literal, $method:literal, $params:literal, $symbol:literal) => {
        OperatorContract {
            interface: $iface,
            method: $method,
            params: $params,
            symbol: $symbol,
        }
    };
}

pub const OP_ADD: OperatorContract = contract!("OpAdd", "add", 1, "+");
pub const OP_SUB: OperatorContract = contract!("OpSub", "sub", 1, "-");
pub const OP_MUL: OperatorContract = contract!("OpMul", "mul", 1, "*");
pub const OP_DIV: OperatorContract = contract!("OpDiv", "div", 1, "/");
pub const OP_MOD: OperatorContract = contract!("OpMod", "mod", 1, "%");
pub const OP_POW: OperatorContract = contract!("OpPow", "pow", 1, "^");
pub const OP_BAND: OperatorContract = contract!("OpBAnd", "band", 1, "&");
pub const OP_BOR: OperatorContract = contract!("OpBOr", "bor", 1, "|");
pub const OP_BXOR: OperatorContract = contract!("OpBXor", "bxor", 1, "~");
pub const OP_SHL: OperatorContract = contract!("OpShl", "shl", 1, "<<");
pub const OP_SHR: OperatorContract = contract!("OpShr", "shr", 1, ">>");
pub const OP_NEG: OperatorContract = contract!("OpNeg", "neg", 0, "-");
pub const OP_BNOT: OperatorContract = contract!("OpBNot", "bnot", 0, "~");
pub const OP_LEN: OperatorContract = contract!("OpLen", "len", 0, "#");
pub const OP_CONCAT: OperatorContract = contract!("OpConcat", "concat", 1, "..");
pub const OP_EQ: OperatorContract = contract!("OpEq", "equals", 1, "==");
pub const OP_COMPARE: OperatorContract = contract!("OpCompare", "compare", 1, "<");
pub const OP_TO_STRING: OperatorContract = contract!("OpToString", "toString", 0, "tostring");

/// Every operator contract, in declaration order. Used to seed the
/// interface registries.
pub const OPERATOR_CONTRACTS: &[OperatorContract] = &[
    OP_ADD,
    OP_SUB,
    OP_MUL,
    OP_DIV,
    OP_MOD,
    OP_POW,
    OP_BAND,
    OP_BOR,
    OP_BXOR,
    OP_SHL,
    OP_SHR,
    OP_NEG,
    OP_BNOT,
    OP_LEN,
    OP_CONCAT,
    OP_EQ,
    OP_COMPARE,
    OP_TO_STRING,
];

/// The contract a class must satisfy to overload `op`.
///
/// `None` for `and` / `or` / `??`: they act on truthiness and nil-ness,
/// which every value already answers, so there is nothing to overload.
pub fn binary_contract(op: BinOp) -> Option<OperatorContract> {
    use BinOp::*;
    Some(match op {
        Add => OP_ADD,
        Sub => OP_SUB,
        Mul => OP_MUL,
        Div => OP_DIV,
        Mod => OP_MOD,
        Pow => OP_POW,
        BAnd => OP_BAND,
        BOr => OP_BOR,
        BXor => OP_BXOR,
        Shl => OP_SHL,
        Shr => OP_SHR,
        Concat => OP_CONCAT,
        // `!=` is `==` negated rather than its own contract, mirroring Lua.
        Eq | NotEq => OP_EQ,
        // One `compare` covers all four ordering operators.
        Lt | LtEq | Gt | GtEq => OP_COMPARE,
        And | Or | Coalesce => return None,
    })
}

/// The contract a class must satisfy to overload unary `op`.
///
/// `not x` is deliberately absent — truthiness stays a property of the
/// value, matching Lua's lack of a `__not` metamethod.
pub fn unary_contract(op: UnaryOp) -> Option<OperatorContract> {
    match op {
        UnaryOp::Neg => Some(OP_NEG),
        UnaryOp::BNot => Some(OP_BNOT),
        UnaryOp::Len => Some(OP_LEN),
        UnaryOp::Not => None,
    }
}

/// Is `name` one of the built-in operator interfaces?
pub fn is_operator_interface(name: &str) -> bool {
    OPERATOR_CONTRACTS.iter().any(|c| c.interface == name)
}

/// The operator as written in source. Unlike [`OperatorContract::symbol`]
/// this covers the non-overloadable operators too.
pub fn binop_symbol(op: BinOp) -> &'static str {
    use BinOp::*;
    match op {
        Add => "+",
        Sub => "-",
        Mul => "*",
        Div => "/",
        Mod => "%",
        Pow => "^",
        BAnd => "&",
        BOr => "|",
        BXor => "~",
        Shl => "<<",
        Shr => ">>",
        Eq => "==",
        NotEq => "!=",
        Lt => "<",
        LtEq => "<=",
        Gt => ">",
        GtEq => ">=",
        And => "and",
        Or => "or",
        Concat => "..",
        Coalesce => "??",
    }
}
