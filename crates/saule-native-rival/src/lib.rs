//! `rival` — a native package that exports a class with the same name as
//! one in `fixture`, `Util`, with a different `greet`.
//!
//! Installed beside the fixture, it is how the tests prove that a program
//! is checked against the package it imported. Static signatures used to
//! live in one table keyed by class name, so with both installed whichever
//! package was registered last decided what `Util.greet` took, and that
//! changed from one run to the next.

use saule_sdk::prelude::*;

saule_package! {
    name = "rival",
    version = "0.1.0",
    doc = "Shares a class name with `fixture`, and nothing else.",
    classes {
        Util = "Not the fixture's `Util`.",
    }
}

/// Takes nothing, where the fixture's `Util.greet` needs a name.
#[saule_export(class = "Util")]
fn greet() -> String {
    "hello from rival".to_string()
}
