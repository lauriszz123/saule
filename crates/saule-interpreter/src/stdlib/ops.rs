//! Built-in operator interfaces — Saule's answer to Lua's metamethods.
//!
//! Each interface names one operator and declares the single method that
//! implements it. A class opts in by listing the interface in its
//! `implements` clause and defining the method:
//!
//! ```saule
//! export class Vec2 implements OpAdd<Vec2, Vec2>, OpToString
//!     local x: float
//!     local y: float
//!
//!     fn add(other: Vec2) -> Vec2
//!         return Vec2(self.x + other.x, self.y + other.y)
//!     end
//!
//!     fn toString() -> string
//!         return "(" .. self.x .. ", " .. self.y .. ")"
//!     end
//! end
//! ```
//!
//! | Interface        | Operator        | Method                          |
//! |------------------|-----------------|---------------------------------|
//! | `OpAdd<T, R>`    | `a + b`         | `fn add(other: T) -> R`         |
//! | `OpSub<T, R>`    | `a - b`         | `fn sub(other: T) -> R`         |
//! | `OpMul<T, R>`    | `a * b`         | `fn mul(other: T) -> R`         |
//! | `OpDiv<T, R>`    | `a / b`         | `fn div(other: T) -> R`         |
//! | `OpMod<T, R>`    | `a % b`         | `fn mod(other: T) -> R`         |
//! | `OpPow<T, R>`    | `a ^ b`         | `fn pow(other: T) -> R`         |
//! | `OpBAnd<T, R>`   | `a & b`         | `fn band(other: T) -> R`        |
//! | `OpBOr<T, R>`    | `a \| b`        | `fn bor(other: T) -> R`         |
//! | `OpBXor<T, R>`   | `a ~ b`         | `fn bxor(other: T) -> R`        |
//! | `OpShl<T, R>`    | `a << b`        | `fn shl(other: T) -> R`         |
//! | `OpShr<T, R>`    | `a >> b`        | `fn shr(other: T) -> R`         |
//! | `OpNeg<R>`       | `-a`            | `fn neg() -> R`                 |
//! | `OpBNot<R>`      | `~a`            | `fn bnot() -> R`                |
//! | `OpLen`          | `#a`            | `fn len() -> integer`           |
//! | `OpConcat<T, R>` | `a .. b`        | `fn concat(other: T) -> R`      |
//! | `OpEq<T>`        | `a == b`, `!=`  | `fn equals(other: T) -> boolean`|
//! | `OpCompare<T>`   | `<` `<=` `>` `>=` | `fn compare(other: T) -> integer` |
//! | `OpToString`     | `tostring(a)`   | `fn toString() -> string`       |
//!
//! The interfaces carry no behaviour of their own — the dispatch lives in
//! [`crate::eval::ops`], which looks the contract method up on the
//! instance's class chain. What the `implements` clause buys is the static
//! check: `saule-typeck` only accepts an operator on a class that declares
//! the matching interface, and reads the result type off the method.

use crate::fxhash::fxmap;
use std::cell::RefCell;
use std::rc::Rc;

use crate::env::Environment;
use crate::native_packages::NativePackage;
use crate::value::{InterfaceObject, Value};

/// `import OpAdd, OpCompare from "ops"`. Auto-prelude'd so operators can
/// be overloaded without an import, the same way `Iterable` is always in
/// scope for `for … in`.
///
/// `exports` has to be a literal `&'static [&'static str]`, so it spells
/// the names out rather than deriving them from
/// [`saule_ast::ops::OPERATOR_CONTRACTS`]. `exports_match_contracts` below
/// keeps the two in step.
pub static OPS_PACKAGE: NativePackage = NativePackage {
    name: "ops",
    version: env!("CARGO_PKG_VERSION"),
    install,
    exports: &[
        "OpAdd",
        "OpSub",
        "OpMul",
        "OpDiv",
        "OpMod",
        "OpPow",
        "OpBAnd",
        "OpBOr",
        "OpBXor",
        "OpShl",
        "OpShr",
        "OpNeg",
        "OpBNot",
        "OpLen",
        "OpConcat",
        "OpEq",
        "OpCompare",
        "OpToString",
    ],
    register_sigs,
    builtins: empty_builtins,
    auto_prelude: true,
};

fn empty_builtins() -> saule_semantic::builtins::Builtins {
    saule_semantic::builtins::Builtins::default()
}

pub fn install(env: &Rc<RefCell<Environment>>) {
    for contract in saule_ast::ops::OPERATOR_CONTRACTS {
        let mut methods = fxmap();
        methods.insert(contract.method.to_string(), (contract.params, true));
        env.borrow_mut().define(
            contract.interface.to_string(),
            Value::Interface(Rc::new(InterfaceObject {
                name: contract.interface.to_string(),
                extends: Vec::new(),
                methods,
            })),
        );
    }
}

/// Register native signatures for the typechecker (lazy, via `sigs::lookup`).
pub fn register_sigs() {
    // Nothing — operator interfaces have no native call sites. The result
    // type of `a + b` is read off the implementing class's own `add`
    // signature, which the class registry already carries.
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A contract missing from `exports` would still be installed into the
    /// environment but wouldn't reach the name resolver's prelude, so
    /// `implements OpX` would fail as an undefined name.
    #[test]
    fn exports_match_contracts() {
        let declared: Vec<&str> = saule_ast::ops::OPERATOR_CONTRACTS
            .iter()
            .map(|c| c.interface)
            .collect();
        assert_eq!(declared, OPS_PACKAGE.exports);
    }
}
