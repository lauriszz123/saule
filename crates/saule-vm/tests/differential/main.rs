//! Differential testing: every program is run under **both** engines and the
//! results compared (`VM_DESIGN.md` §23.2).
//!
//! This is the highest-value test shape for the project. The tree-walker is
//! the oracle — it is ~13k lines that already work and it defines what the
//! language means — so "the VM agrees with it" is a much stronger statement
//! than any hand-written expectation, and it costs nothing to author.
//!
//! Programs the compiler cannot handle yet are skipped rather than failed:
//! `CompileError::Unsupported` is the designed signal for "fall back to the
//! tree-walker" (§21.3), so treating it as a failure would make every
//! not-yet-written feature look like a bug.
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
