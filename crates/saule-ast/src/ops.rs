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
    /// Generic type parameters the *interface* declares, as documented:
    /// `OpAdd<T, R>` has two, `OpEq<T>` one, `OpLen` none.
    ///
    /// These interfaces are built in, so no declaration exists for the
    /// checker to read an arity off. Recording it here is what lets
    /// `implements OpAdd<Vec2>` be reported as the missing result type it
    /// is, rather than passing because nothing knew better.
    pub type_params: usize,
    /// The operator as written in source, for diagnostics.
    pub symbol: &'static str,
}

macro_rules! contract {
    ($iface:literal, $method:literal, $params:literal, $symbol:literal, $tps:literal) => {
        OperatorContract {
            interface: $iface,
            method: $method,
            params: $params,
            symbol: $symbol,
            type_params: $tps,
        }
    };
}

pub const OP_ADD: OperatorContract = contract!("OpAdd", "add", 1, "+", 2);
pub const OP_SUB: OperatorContract = contract!("OpSub", "sub", 1, "-", 2);
pub const OP_MUL: OperatorContract = contract!("OpMul", "mul", 1, "*", 2);
pub const OP_DIV: OperatorContract = contract!("OpDiv", "div", 1, "/", 2);
pub const OP_MOD: OperatorContract = contract!("OpMod", "mod", 1, "%", 2);
pub const OP_POW: OperatorContract = contract!("OpPow", "pow", 1, "^", 2);
pub const OP_BAND: OperatorContract = contract!("OpBAnd", "band", 1, "&", 2);
pub const OP_BOR: OperatorContract = contract!("OpBOr", "bor", 1, "|", 2);
pub const OP_BXOR: OperatorContract = contract!("OpBXor", "bxor", 1, "~", 2);
pub const OP_SHL: OperatorContract = contract!("OpShl", "shl", 1, "<<", 2);
pub const OP_SHR: OperatorContract = contract!("OpShr", "shr", 1, ">>", 2);
pub const OP_NEG: OperatorContract = contract!("OpNeg", "neg", 0, "-", 1);
pub const OP_BNOT: OperatorContract = contract!("OpBNot", "bnot", 0, "~", 1);
pub const OP_LEN: OperatorContract = contract!("OpLen", "len", 0, "#", 0);
pub const OP_CONCAT: OperatorContract = contract!("OpConcat", "concat", 1, "..", 2);
pub const OP_EQ: OperatorContract = contract!("OpEq", "equals", 1, "==", 1);
pub const OP_COMPARE: OperatorContract = contract!("OpCompare", "compare", 1, "<", 1);
pub const OP_TO_STRING: OperatorContract = contract!("OpToString", "toString", 0, "tostring", 0);

// ── Behaviour contracts ──────────────────────────────────────────────────────
//
// Not operators: no source symbol triggers them, so `binary_contract` and
// `unary_contract` never return one. They share [`OperatorContract`] because
// what the registries need — an interface name, a method, an arity — is
// identical, and every registration site iterates [`ALL_CONTRACTS`].

/// `obj[key]` on a class instance. Saule's `__index`.
pub const OP_INDEX: OperatorContract = contract!("OpIndex", "index", 1, "[]", 2);
/// `obj[key] = value` on a class instance. Saule's `__newindex`.
pub const OP_NEW_INDEX: OperatorContract = contract!("OpNewIndex", "newIndex", 2, "[]=", 2);
/// `Class.of(value)` — and the same call applied *implicitly* wherever a
/// declared type asks for the class and a bare value is supplied.
///
/// The method is **static**, unlike every other contract here: there is no
/// instance yet to call it on.
pub const ASSIGNABLE: OperatorContract = contract!("Assignable", "of", 1, "of", 1);

/// Every operator contract, in declaration order.
///
/// Operators only: this is what [`binary_contract`] / [`unary_contract`] map
/// into, so a class listing one of these is opting into a *symbol*. The
/// behaviour contracts live in [`ALL_CONTRACTS`] alongside them.
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

/// The behaviour contracts — everything in [`ALL_CONTRACTS`] that no operator
/// symbol maps to.
pub const BEHAVIOUR_CONTRACTS: &[OperatorContract] = &[OP_INDEX, OP_NEW_INDEX, ASSIGNABLE];

/// Every built-in contract a class can implement, operators first.
///
/// This is what the registries seed from: `saule-semantic` pre-registers the
/// names so `implements Assignable<…>` resolves without an import, and
/// `saule-interpreter`'s `ops` package installs one interface value per entry.
pub const ALL_CONTRACTS: &[OperatorContract] = &[
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
    OP_INDEX,
    OP_NEW_INDEX,
    ASSIGNABLE,
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

/// Is `name` one of the built-in interfaces — operator or behaviour?
pub fn is_operator_interface(name: &str) -> bool {
    ALL_CONTRACTS.iter().any(|c| c.interface == name)
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
