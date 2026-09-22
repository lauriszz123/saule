//! Modules: where an `import` points, and what the importing module may
//! assume about it before anything runs.
//!
//! Each `.sau` (or `.saule`) file is a *module*. The bytecode VM's program
//! driver resolves a whole program's import graph at compile time and
//! compiles every module in it; what lives here is the part it shares with
//! the checker and the language server:
//!
//! * [`resolve_import_path`] — the file (or native-package sentinel) an
//!   `import` names, and [`is_init_module`], which decides which modules
//!   re-export what they import;
//! * [`collect_import_seed`] and friends — the classes, interfaces and enums
//!   an import brings into scope, read from the imported file's source so a
//!   module can be checked without running anything.

mod resolve;
mod seed;
#[cfg(test)]
mod tests;

pub use resolve::*;
pub use seed::*;

use std::collections::HashMap;

use saule_ast::Decl;

use crate::value::Value;

/// The publicly importable surface of a native package: its exported names
/// and the values they are bound to.
#[derive(Debug, Default, Clone)]
pub struct ModuleExports {
    pub values: HashMap<String, Value>,
}

/// The name a declaration publishes, if it carries `export`. Anything not
/// exported stays private to its module.
fn exported_name(decl: &Decl) -> Option<&str> {
    match decl {
        Decl::Function {
            exported: true,
            name,
            ..
        }
        | Decl::Class {
            exported: true,
            name,
            ..
        }
        | Decl::Interface {
            exported: true,
            name,
            ..
        }
        | Decl::Enum {
            exported: true,
            name,
            ..
        }
        | Decl::Variable {
            exported: true,
            name,
            ..
        } => Some(name),
        _ => None,
    }
}
