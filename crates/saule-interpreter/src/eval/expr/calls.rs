//! Calling user functions, native functions, instance/static methods, and
//! dispatching `obj.member(args)` calls.

mod binding;
mod invoke;
mod methods;

pub(crate) use binding::*;
pub(crate) use invoke::*;
pub(crate) use methods::*;

// Re-export the helpers `eval` in mod.rs needs to call.
pub(super) use call_value as call_value_pub;
pub(super) use eval_call_args as eval_call_args_pub;
