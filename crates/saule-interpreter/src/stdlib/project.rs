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

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::env::Environment;
use crate::value::{ClassObject, FieldDef, TableObject, Value};

pub fn install(env: &Rc<RefCell<Environment>>) {
    let info = crate::project::get().unwrap_or_default();

    let src_dirs: Vec<Value> = info
        .src_dirs
        .iter()
        .map(|p| Value::Str(Rc::new(p.display().to_string())))
        .collect();

    let mut static_fields = HashMap::new();
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
        methods: HashMap::new(),
        static_fields: RefCell::new(static_fields),
        static_methods: HashMap::new(),
        constructor: None,
    };
    env.borrow_mut()
        .define("Project".to_string(), Value::Class(Rc::new(class)));
}

/// Register native signatures for the typechecker.
pub fn register_sigs() {
    // Static fields are looked up as regular class member access; no native
    // method signatures to register today. Kept for parity with other
    // stdlib modules so future `Project.foo()` methods slot in cleanly.
}
