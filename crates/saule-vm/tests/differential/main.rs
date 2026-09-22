//! Conformance: every program is run on the VM and its outcome — value or
//! error text — compared with the one recorded for it (`VM_DESIGN.md`
//! §23.2).
//!
//! This began as differential testing, with the tree-walking interpreter as
//! the oracle: it defined what the language meant, so "the VM agrees with
//! it" was a stronger statement than any hand-written expectation. The
//! tree-walker has since been removed, and its answers kept — every outcome
//! in `expected.txt` was recorded from it, when it and the VM agreed. See
//! `harness.rs` for how a new test records its own.
//!
//! One file per feature area; the shared harness lives in `harness.rs` and is
//! the only thing they have in common.

mod harness;

mod arg_binding;
mod assignable;
mod basics;
mod classes;
mod control_flow;
mod dynamic;
mod enums_match;
mod errors;
mod functions;
mod iteration;
mod modules;
mod multi_return;
mod nullability;
mod peepholes;
mod pipes;
mod reentrancy;
mod smoke;
mod tables;
