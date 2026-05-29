//! Compatibility shim — the native signature registry was moved to
//! `saule-typeck`. This module just re-exports the public surface so the
//! per-stdlib `register_sigs` functions can keep importing
//! `crate::stdlib::sigs::{register, t_named, ...}` unchanged.

pub use saule_typeck::sigs::{
    NativeSig, lookup, register, register_member, register_v, set_initializer, t_any, t_function,
    t_named, t_nullable, t_number, t_table, t_table_map,
};
