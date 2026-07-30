//! `Project` static class — exposes the active `saule.config` to user code.
//!
//! Fields:
//!   * `name: string`         — project name (empty in single-file mode)
//!   * `version: string`      — project version
//!   * `root: string`         — absolute path of the project root
//!   * `srcDirs: table<string>` — extra source roots resolved against root
//!
//! All values are *snapshots* taken at startup. There is no setter; mutating
//! the table doesn't propagate back to the loader.

use crate::fxhash::fxmap;
use std::cell::RefCell;
use std::rc::Rc;

use crate::env::Environment;
use crate::native_packages::NativePackage;
use crate::value::{ClassObject, FieldDef, TableObject, Value};

/// `import Project from "project"`. Auto-prelude'd so the existing bare
/// `Project.name` references keep working.
pub static PROJECT_PACKAGE: NativePackage = NativePackage {
    name: "project",
    version: saule_version::VERSION,
    install,
    exports: &["Project"],
    register_sigs,
    builtins: empty_builtins,
    auto_prelude: true,
};

fn empty_builtins() -> saule_semantic::builtins::Builtins {
    saule_semantic::builtins::Builtins::default()
}

pub fn install(env: &Rc<RefCell<Environment>>) {
    let info = crate::project::get().unwrap_or_default();

    let src_dirs: Vec<Value> = info
        .src_dirs
        .iter()
        .map(|p| Value::Str(Rc::new(p.display().to_string())))
        .collect();

    let mut static_fields = fxmap();
    static_fields.insert("name".to_string(), Value::Str(Rc::new(info.name.clone())));
    static_fields.insert(
        "version".to_string(),
        Value::Str(Rc::new(info.version.clone())),
    );
    static_fields.insert(
        "root".to_string(),
        Value::Str(Rc::new(info.root.display().to_string())),
    );
    static_fields.insert(
        "srcDirs".to_string(),
        Value::Table(Rc::new(RefCell::new(TableObject::from_array(src_dirs)))),
    );

    let class = ClassObject {
        name: "Project".to_string(),
        parent: None,
        field_defs: Vec::<FieldDef>::new(),
        methods: Default::default(),
        static_fields: RefCell::new(static_fields),
        static_methods: Default::default(),
        constructor: None,
    };
    env.borrow_mut()
        .define("Project".to_string(), Value::Class(Rc::new(class)));
}

/// Register native signatures for the typechecker.
pub fn register_sigs() {
    use crate::stdlib::sigs::{register_const, t_named};
    use saule_ast::Type;

    // Field-only module — no callables. `install` always defines these
    // (falling back to a default `ProjectInfo` in single-file mode), so
    // they are unconditionally present and unconditionally typed.
    let s = || t_named("string");
    register_const("Project.name", s());
    register_const("Project.version", s());
    register_const("Project.root", s());
    register_const(
        "Project.srcDirs",
        Type::Table {
            key: None,
            value: Box::new(s()),
        },
    );
}
