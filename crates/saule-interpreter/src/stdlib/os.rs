//! `Os` static class + `OsPlatform` enum.
//!
//! Design choices:
//!
//! * Failure modes use the Saule idiom: filesystem mutations
//!   (`remove`/`rename`/`chdir`/`mkdir`) return `boolean`, lookups that may
//!   miss (`getenv`) return `string?`. Nothing throws — calling code is
//!   expected to read the result.
//!
//! * `Os.date` always returns a `string` (no Lua-style polymorphic
//!   string-or-table return). The format follows `strftime`-ish placeholders
//!   handled inline: `%Y %m %d %H %M %S %y %j %w %c %x %X %%`.
//!
//! * `Os.platform()` returns an `OsPlatform` enum, mirroring `IoMode` —
//!   no magic strings at call sites.
//!
//! * `Os.execute` collapses Lua's `(ok, signal, code)` triple into a single
//!   integer exit code (0 on success). A richer captured-output variant can
//!   be added later as `Os.run`.

use crate::fxhash::fxmap;
use std::cell::RefCell;
use std::rc::Rc;

use crate::env::Environment;
use crate::native_packages::NativePackage;
use crate::value::{
    ClassObject, EnumObject, EnumVariantObject, FieldDef, InstanceObject, NativeClosure,
    TableObject, Value,
};

/// `import Os, OsPlatform, FsKind, FsInfo from "os"`. Auto-prelude'd so
/// the existing bare-name call sites keep working.
pub static OS_PACKAGE: NativePackage = NativePackage {
    name: "os",
    version: env!("CARGO_PKG_VERSION"),
    install,
    exports: &["Os", "OsPlatform", "FsKind", "FsInfo"],
    register_sigs,
    builtins: os_builtins,
    auto_prelude: true,
};

fn os_builtins() -> saule_semantic::builtins::Builtins {
    let (classes, interfaces, enums) = builtin_registries();
    saule_semantic::builtins::Builtins {
        classes,
        interfaces,
        enums,
    }
}

thread_local! {
    /// Phantom class object reused as the type tag for every `FsInfo`
    /// instance produced by `Os.fsInfo`. Populated by `install`.
    static FSINFO_CLASS: RefCell<Option<Rc<ClassObject>>> = const { RefCell::new(None) };
}

// ─── installation ──────────────────────────────────────────────────────────

pub fn install(env: &Rc<RefCell<Environment>>) {
    install_platform_enum(env);
    install_fskind_enum(env);
    install_fsinfo_class(env);

    let mut static_fields = fxmap();

    // time
    static_fields.insert("time".to_string(), native_multi("Os.time", os_time));
    static_fields.insert("clock".to_string(), native_multi("Os.clock", os_clock));
    static_fields.insert(
        "difftime".to_string(),
        native_multi("Os.difftime", os_difftime),
    );
    static_fields.insert("date".to_string(), native_multi("Os.date", os_date));
    static_fields.insert(
        "parsedate".to_string(),
        native_multi("Os.parsedate", os_parsedate),
    );
    static_fields.insert("sleep".to_string(), native_multi("Os.sleep", os_sleep));

    // environment
    static_fields.insert("getenv".to_string(), native_multi("Os.getenv", os_getenv));
    static_fields.insert("setenv".to_string(), native_multi("Os.setenv", os_setenv));
    static_fields.insert("cwd".to_string(), native_multi("Os.cwd", os_cwd));
    static_fields.insert("chdir".to_string(), native_multi("Os.chdir", os_chdir));

    // filesystem
    static_fields.insert("remove".to_string(), native_multi("Os.remove", os_remove));
    static_fields.insert("rename".to_string(), native_multi("Os.rename", os_rename));
    static_fields.insert("list".to_string(), native_multi("Os.list", os_list));
    static_fields.insert("exists".to_string(), native_multi("Os.exists", os_exists));
    static_fields.insert("fsInfo".to_string(), native_multi("Os.fsInfo", os_fs_info));
    static_fields.insert("mkdir".to_string(), native_multi("Os.mkdir", os_mkdir));
    static_fields.insert(
        "tmpname".to_string(),
        native_multi("Os.tmpname", os_tmpname),
    );

    // process
    static_fields.insert("exit".to_string(), native_multi("Os.exit", os_exit));
    static_fields.insert(
        "execute".to_string(),
        native_multi("Os.execute", os_execute),
    );
    static_fields.insert("pid".to_string(), native_multi("Os.pid", os_pid));
    static_fields.insert(
        "platform".to_string(),
        native_multi("Os.platform", os_platform),
    );
    static_fields.insert("args".to_string(), native_multi("Os.args", os_args));

    // constants
    static_fields.insert(
        "sep".to_string(),
        Value::Str(Rc::new(path_sep().to_string())),
    );
    static_fields.insert(
        "lineSep".to_string(),
        Value::Str(Rc::new(line_sep().to_string())),
    );

    let class = ClassObject {
        name: "Os".to_string(),
        parent: None,
        field_defs: Vec::<FieldDef>::new(),
        methods: Default::default(),
        static_fields: RefCell::new(static_fields),
        static_methods: Default::default(),
        constructor: None,
    };
    env.borrow_mut()
        .define("Os".to_string(), Value::Class(Rc::new(class)));
}

/// Register native signatures for the typechecker (lazy, via `sigs::lookup`).
pub fn register_sigs() {
    use crate::stdlib::sigs::{register, t_named, t_nullable, t_number};
    use saule_ast::Type;
    let s = || t_named("string");
    let i = || t_named("integer");
    let f = || t_named("float");
    let b = || t_named("boolean");
    let nil = || t_named("nil");
    let str_opt = || t_nullable(s());
    let table_str = || Type::Table {
        key: None,
        value: Box::new(s()),
    };

    // time
    register("Os.time", vec![], vec![i()]);
    register("Os.clock", vec![], vec![f()]);
    register("Os.difftime", vec![i(), i()], vec![i()]);
    register("Os.date", vec![t_nullable(s()), t_nullable(i())], vec![s()]);
    // Parse a date string into a unix epoch. Returns nil when the input
    // doesn't match the format, so the result type is `integer?`.
    register(
        "Os.parsedate",
        vec![s(), t_nullable(s())],
        vec![t_nullable(i())],
    );
    register("Os.sleep", vec![t_number()], vec![nil()]);

    // environment
    register("Os.getenv", vec![s()], vec![str_opt()]);
    register("Os.setenv", vec![s(), s()], vec![nil()]);
    register("Os.cwd", vec![], vec![s()]);
    register("Os.chdir", vec![s()], vec![b()]);

    // filesystem
    register("Os.remove", vec![s()], vec![b()]);
    register("Os.rename", vec![s(), s()], vec![b()]);
    register("Os.list", vec![s()], vec![table_str()]);
    register("Os.exists", vec![s()], vec![b()]);
    register(
        "Os.fsInfo",
        vec![t_nullable(s())],
        vec![t_nullable(t_named("FsInfo"))],
    );
    register("Os.mkdir", vec![s(), t_nullable(b())], vec![b()]);
    register("Os.tmpname", vec![], vec![s()]);

    // process
    register("Os.exit", vec![t_nullable(i())], vec![nil()]);
    register("Os.execute", vec![s()], vec![i()]);
    register("Os.pid", vec![], vec![i()]);
    register("Os.platform", vec![], vec![t_named("OsPlatform")]);
    // `Os.args() -> table<string>` — process argv. Not generic: the
    // runtime always produces a string-valued table, so a generic `T`
    // would just be a hole that defeats element-type checking at the
    // call site (e.g. `local a: table<Foo> = Os.args()` would pass).
    register("Os.args", vec![], vec![table_str()]);

    // String-valued constants. Typed rather than merely name-recorded, so
    // `local s: string = Os.sep` checks instead of failing as an
    // undetermined type.
    use crate::stdlib::sigs::register_const;
    register_const("Os.sep", s());
    register_const("Os.lineSep", s());
}

// ─── enum ──────────────────────────────────────────────────────────────────

fn install_platform_enum(env: &Rc<RefCell<Environment>>) {
    let variants = &[
        ("Linux", "linux"),
        ("Macos", "macos"),
        ("Windows", "windows"),
        ("Other", "other"),
    ];
    let name = "OsPlatform";
    let mut variant_dict = fxmap();
    for (vname, vvalue) in variants {
        variant_dict.insert(
            (*vname).to_string(),
            Rc::new(EnumVariantObject {
                enum_name: name.to_string(),
                variant_name: (*vname).to_string(),
                value: Some(Value::Str(Rc::new((*vvalue).to_string()))),
                enum_obj: RefCell::new(None),
            }),
        );
    }
    let final_enum = Rc::new(EnumObject {
        name: name.to_string(),
        variants: variant_dict.clone(),
        tuple_variants: Default::default(),
        methods: Default::default(),
    });
    for v in variant_dict.values() {
        *v.enum_obj.borrow_mut() = Some(final_enum.clone());
    }
    env.borrow_mut()
        .define(name.to_string(), Value::Enum(final_enum));
}

// ─── argv passthrough ──────────────────────────────────────────────────────

thread_local! {
    static SCRIPT_ARGS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// Tie-breaker for `Os.tmpname` so it stays unique even on a host that
    /// offers neither a clock nor a pid.
    static TMP_SEQ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Called from the CLI before running user code to publish argv to `Os.args()`.
pub fn set_script_args(args: Vec<String>) {
    SCRIPT_ARGS.with(|cell| *cell.borrow_mut() = args);
}

// ─── native helpers ────────────────────────────────────────────────────────

fn native_multi(name: &'static str, func: fn(&[Value]) -> Result<Vec<Value>, String>) -> Value {
    Value::NativeClosure(Rc::new(NativeClosure {
        name,
        func: Box::new(move |args| func(args)),
        param_names: Vec::new(),
    }))
}

fn expect_string(name: &str, args: &[Value], idx: usize) -> Result<String, String> {
    match args.get(idx) {
        Some(Value::Str(s)) => Ok((**s).clone()),
        Some(other) => Err(format!(
            "{name} expects a string at argument {}, got `{}`",
            idx + 1,
            other.type_name()
        )),
        None => Err(format!("{name} missing argument {}", idx + 1)),
    }
}

fn expect_integer(name: &str, args: &[Value], idx: usize) -> Result<i64, String> {
    match args.get(idx) {
        Some(Value::Int(n)) => Ok(*n),
        Some(other) => Err(format!(
            "{name} expects an integer at argument {}, got `{}`",
            idx + 1,
            other.type_name()
        )),
        None => Err(format!("{name} missing argument {}", idx + 1)),
    }
}

fn str_value(s: String) -> Value {
    Value::Str(Rc::new(s))
}

fn bool_vec(b: bool) -> Vec<Value> {
    vec![Value::Bool(b)]
}

fn nil_vec() -> Vec<Value> {
    vec![Value::Nil]
}

#[cfg(target_os = "linux")]
fn platform_str() -> &'static str {
    "linux"
}
#[cfg(target_os = "macos")]
fn platform_str() -> &'static str {
    "macos"
}
#[cfg(target_os = "windows")]
fn platform_str() -> &'static str {
    "windows"
}
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn platform_str() -> &'static str {
    "other"
}

#[cfg(target_family = "windows")]
fn path_sep() -> &'static str {
    "\\"
}
#[cfg(not(target_family = "windows"))]
fn path_sep() -> &'static str {
    "/"
}

#[cfg(target_family = "windows")]
fn line_sep() -> &'static str {
    "\r\n"
}
#[cfg(not(target_family = "windows"))]
fn line_sep() -> &'static str {
    "\n"
}

// ─── time ──────────────────────────────────────────────────────────────────

fn os_time(_args: &[Value]) -> Result<Vec<Value>, String> {
    let secs = crate::platform::unix_time_secs()
        .ok_or_else(|| crate::platform::unavailable("Os.time"))?;
    Ok(vec![Value::Int(secs as i64)])
}

fn os_clock(_args: &[Value]) -> Result<Vec<Value>, String> {
    let elapsed = crate::platform::monotonic_secs()
        .ok_or_else(|| crate::platform::unavailable("Os.clock"))?;
    Ok(vec![Value::Float(elapsed)])
}

fn os_difftime(args: &[Value]) -> Result<Vec<Value>, String> {
    let t2 = expect_integer("Os.difftime", args, 0)?;
    let t1 = expect_integer("Os.difftime", args, 1)?;
    Ok(vec![Value::Int(t2 - t1)])
}

fn os_date(args: &[Value]) -> Result<Vec<Value>, String> {
    let format = match args.first() {
        Some(Value::Str(s)) => (**s).clone(),
        Some(Value::Nil) | None => "%c".to_string(),
        Some(other) => {
            return Err(format!(
                "Os.date: format must be a string, got `{}`",
                other.type_name()
            ));
        }
    };
    let epoch = match args.get(1) {
        Some(Value::Int(n)) => *n,
        // Only the implicit "now" needs a clock — `Os.date("%Y", 0)` formats
        // a caller-supplied instant and works anywhere.
        Some(Value::Nil) | None => crate::platform::unix_time_secs()
            .ok_or_else(|| crate::platform::unavailable("Os.date with no explicit time"))?
            as i64,
        Some(other) => {
            return Err(format!(
                "Os.date: time must be an integer, got `{}`",
                other.type_name()
            ));
        }
    };
    Ok(vec![str_value(format_epoch(&format, epoch))])
}

fn os_sleep(args: &[Value]) -> Result<Vec<Value>, String> {
    let secs = match args.first() {
        Some(Value::Float(f)) => *f,
        Some(Value::Int(n)) => *n as f64,
        Some(other) => {
            return Err(format!(
                "Os.sleep expects a number, got `{}`",
                other.type_name()
            ));
        }
        None => return Err("Os.sleep missing argument 1".to_string()),
    };
    if secs > 0.0 && secs.is_finite() && !crate::platform::sleep(secs) {
        // Better to say so than to return immediately and leave a program
        // spinning through a loop it expected to be paced.
        return Err(crate::platform::unavailable("Os.sleep"));
    }
    Ok(nil_vec())
}

// ─── environment ───────────────────────────────────────────────────────────

fn os_getenv(args: &[Value]) -> Result<Vec<Value>, String> {
    let name = expect_string("Os.getenv", args, 0)?;
    match std::env::var(&name) {
        Ok(v) => Ok(vec![str_value(v)]),
        Err(_) => Ok(vec![Value::Nil]),
    }
}

fn os_setenv(args: &[Value]) -> Result<Vec<Value>, String> {
    let name = expect_string("Os.setenv", args, 0)?;
    let value = expect_string("Os.setenv", args, 1)?;
    // SAFETY: set_var is process-global and not thread-safe on some
    // platforms. Saule is single-threaded today, so this is sound; revisit
    // when adding `thread`.
    unsafe {
        std::env::set_var(name, value);
    }
    Ok(nil_vec())
}

fn os_cwd(_args: &[Value]) -> Result<Vec<Value>, String> {
    let path = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(vec![str_value(path)])
}

fn os_chdir(args: &[Value]) -> Result<Vec<Value>, String> {
    let path = expect_string("Os.chdir", args, 0)?;
    Ok(bool_vec(std::env::set_current_dir(&path).is_ok()))
}

// ─── filesystem ────────────────────────────────────────────────────────────

fn os_remove(args: &[Value]) -> Result<Vec<Value>, String> {
    let path = expect_string("Os.remove", args, 0)?;
    let p = std::path::Path::new(&path);
    let ok = if p.is_dir() {
        std::fs::remove_dir(p).is_ok()
    } else {
        std::fs::remove_file(p).is_ok()
    };
    Ok(bool_vec(ok))
}

fn os_rename(args: &[Value]) -> Result<Vec<Value>, String> {
    let old = expect_string("Os.rename", args, 0)?;
    let new = expect_string("Os.rename", args, 1)?;
    Ok(bool_vec(std::fs::rename(old, new).is_ok()))
}

fn os_list(args: &[Value]) -> Result<Vec<Value>, String> {
    let path = expect_string("Os.list", args, 0)?;
    let entries = match std::fs::read_dir(&path) {
        Ok(d) => d,
        Err(e) => return Err(format!("Os.list: cannot read `{path}` — {e}")),
    };
    let mut names: Vec<Value> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        names.push(str_value(name));
    }
    Ok(vec![Value::Table(Rc::new(RefCell::new(
        TableObject::from_array(names),
    )))])
}

fn os_exists(args: &[Value]) -> Result<Vec<Value>, String> {
    let path = expect_string("Os.exists", args, 0)?;
    Ok(bool_vec(std::path::Path::new(&path).exists()))
}

fn os_mkdir(args: &[Value]) -> Result<Vec<Value>, String> {
    let path = expect_string("Os.mkdir", args, 0)?;
    let recursive = matches!(args.get(1), Some(Value::Bool(true)));
    let ok = if recursive {
        std::fs::create_dir_all(&path).is_ok()
    } else {
        std::fs::create_dir(&path).is_ok()
    };
    Ok(bool_vec(ok))
}

fn os_tmpname(_args: &[Value]) -> Result<Vec<Value>, String> {
    // Compose a unique-enough path under the platform temp dir. We don't
    // create the file (matching Lua's `os.tmpname` contract), so callers can
    // `Io.open` it with whatever mode they need.
    let dir = std::env::temp_dir();
    // Uniqueness, not accuracy — so this stays total on a host with no clock
    // and no pid, falling back to a per-thread counter. `Os.tmpname` must not
    // be the thing that throws.
    let stamp = crate::platform::unix_time_secs()
        .map(|s| (s * 1e9) as u128)
        .unwrap_or(0);
    let pid = crate::platform::pid().unwrap_or(0);
    let seq = TMP_SEQ.with(|c| {
        let next = c.get() + 1;
        c.set(next);
        next
    });
    let path = dir.join(format!("saule_{pid}_{stamp}_{seq}.tmp"));
    Ok(vec![str_value(path.to_string_lossy().into_owned())])
}

// ─── process ───────────────────────────────────────────────────────────────

fn os_exit(args: &[Value]) -> Result<Vec<Value>, String> {
    let code = match args.first() {
        Some(Value::Int(n)) => *n as i32,
        Some(Value::Nil) | None => 0,
        Some(other) => {
            return Err(format!(
                "Os.exit expects an integer, got `{}`",
                other.type_name()
            ));
        }
    };
    // Natively this diverges. Where the host cannot terminate — a wasm
    // module, which would otherwise be torn down mid-run — it records the
    // code and returns, and we unwind with an error instead. The embedder
    // pairs that error with `platform::take_exit()` to tell a deliberate
    // `Os.exit(0)` from a crash.
    crate::platform::exit(code);
    Err(format!("program exited with code {code}"))
}

fn os_execute(args: &[Value]) -> Result<Vec<Value>, String> {
    let cmd = expect_string("Os.execute", args, 0)?;
    let (program, shell_flag) = if cfg!(target_family = "windows") {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };
    let status = std::process::Command::new(program)
        .arg(shell_flag)
        .arg(&cmd)
        .status();
    let code = match status {
        Ok(s) => s.code().unwrap_or(-1) as i64,
        Err(_) => -1,
    };
    Ok(vec![Value::Int(code)])
}

fn os_pid(_args: &[Value]) -> Result<Vec<Value>, String> {
    // 0 rather than an error: "no process" is a truthful answer for a
    // sandboxed host, and callers use this for uniqueness, not control.
    let pid = crate::platform::pid().unwrap_or(0);
    Ok(vec![Value::Int(pid as i64)])
}

fn os_platform(_args: &[Value]) -> Result<Vec<Value>, String> {
    let variant_name = match platform_str() {
        "linux" => "Linux",
        "macos" => "Macos",
        "windows" => "Windows",
        _ => "Other",
    };
    let variant = EnumVariantObject {
        enum_name: "OsPlatform".to_string(),
        variant_name: variant_name.to_string(),
        value: Some(str_value(platform_str().to_string())),
        enum_obj: RefCell::new(None),
    };
    Ok(vec![Value::EnumVariant(Rc::new(variant))])
}

fn os_args(_args: &[Value]) -> Result<Vec<Value>, String> {
    let argv: Vec<Value> =
        SCRIPT_ARGS.with(|cell| cell.borrow().iter().map(|s| str_value(s.clone())).collect());
    Ok(vec![Value::Table(Rc::new(RefCell::new(
        TableObject::from_array(argv),
    )))])
}

// ─── strftime-ish helper ──────────────────────────────────────────────────

/// Implements `%Y %m %d %H %M %S %y %j %w %c %x %X %%`. Unknown specifiers
/// are passed through literally.
fn format_epoch(format: &str, epoch: i64) -> String {
    let (y, mo, d, hh, mm, ss, wday, yday) = civil_from_epoch(epoch);
    let mut out = String::with_capacity(format.len() + 16);
    let mut chars = format.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('Y') => out.push_str(&format!("{y:04}")),
            Some('m') => out.push_str(&format!("{mo:02}")),
            Some('d') => out.push_str(&format!("{d:02}")),
            Some('H') => out.push_str(&format!("{hh:02}")),
            Some('M') => out.push_str(&format!("{mm:02}")),
            Some('S') => out.push_str(&format!("{ss:02}")),
            Some('y') => out.push_str(&format!("{:02}", y % 100)),
            Some('j') => out.push_str(&format!("{yday:03}")),
            Some('w') => out.push_str(&wday.to_string()),
            Some('c') => out.push_str(&format!("{y:04}-{mo:02}-{d:02} {hh:02}:{mm:02}:{ss:02}")),
            Some('x') => out.push_str(&format!("{y:04}-{mo:02}-{d:02}")),
            Some('X') => out.push_str(&format!("{hh:02}:{mm:02}:{ss:02}")),
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

/// Convert a unix epoch (seconds, UTC) to civil time.
/// Returns `(year, month [1-12], day [1-31], hour, minute, second,
/// weekday [0=Sun..6=Sat], yearday [1-366])`.
///
/// Algorithm: Howard Hinnant, "civil_from_days".
fn civil_from_epoch(epoch: i64) -> (i64, u32, u32, u32, u32, u32, u32, u32) {
    let days = epoch.div_euclid(86_400);
    let rem = epoch.rem_euclid(86_400) as u32;
    let hh = rem / 3600;
    let mm = (rem % 3600) / 60;
    let ss = rem % 60;

    // Shift epoch so that day 0 = 0000-03-01 (start of the Gregorian cycle).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y_shifted = yoe as i64 + era * 400;
    let doy = (doe - (365 * yoe + yoe / 4 - yoe / 100)) as u32;
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m_shifted = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m_shifted <= 2 {
        y_shifted + 1
    } else {
        y_shifted
    };

    // Day-of-year via leap-year flag.
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let cum: [u32; 13] = if leap {
        [0, 31, 60, 91, 121, 152, 182, 213, 244, 274, 305, 335, 366]
    } else {
        [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334, 365]
    };
    let yday = cum[(m_shifted - 1) as usize] + d;

    // Weekday: Unix epoch day 0 was a Thursday (4).
    let wday = (((days + 4) % 7 + 7) % 7) as u32;

    (year, m_shifted as u32, d, hh, mm, ss, wday, yday)
}

/// Inverse of `civil_from_epoch`: convert a UTC civil date/time to a unix
/// epoch in seconds. Algorithm: Howard Hinnant, "days_from_civil".
fn epoch_from_civil(y: i64, m: u32, d: u32, hh: u32, mm: u32, ss: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = (y - era * 400) as u64; // [0, 399]
    let m_u = m as u64;
    let d_u = d as u64;
    let doy = (153 * if m_u > 2 { m_u - 3 } else { m_u + 9 } + 2) / 5 + d_u - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    let days = era * 146_097 + doe as i64 - 719_468;
    days * 86_400 + (hh as i64) * 3600 + (mm as i64) * 60 + ss as i64
}

/// `Os.parsedate(input, format?) -> integer?`
///
/// Parses `input` against `format` (default `%Y-%m-%d`) and returns the
/// matching unix epoch in seconds (UTC). Supported specifiers mirror
/// `Os.date`: `%Y %m %d %H %M %S %y %%`. Literal characters in the
/// format must match exactly. Returns `nil` on any mismatch — callers
/// are expected to handle the failure case.
fn os_parsedate(args: &[Value]) -> Result<Vec<Value>, String> {
    let input = expect_string("Os.parsedate", args, 0)?;
    let format = match args.get(1) {
        Some(Value::Str(s)) => (**s).clone(),
        Some(Value::Nil) | None => "%Y-%m-%d".to_string(),
        Some(other) => {
            return Err(format!(
                "Os.parsedate: format must be a string, got `{}`",
                other.type_name()
            ));
        }
    };

    let in_bytes = input.as_bytes();
    let fmt_bytes = format.as_bytes();
    let mut i = 0usize;
    let mut j = 0usize;
    let mut year: i64 = 1970;
    let mut month: u32 = 1;
    let mut day: u32 = 1;
    let mut hour: u32 = 0;
    let mut minute: u32 = 0;
    let mut second: u32 = 0;

    fn take_digits(s: &[u8], i: &mut usize, n: usize) -> Option<u64> {
        if *i + n > s.len() {
            return None;
        }
        let slice = &s[*i..*i + n];
        if !slice.iter().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let mut v: u64 = 0;
        for &c in slice {
            v = v * 10 + (c - b'0') as u64;
        }
        *i += n;
        Some(v)
    }

    while j < fmt_bytes.len() {
        if fmt_bytes[j] != b'%' {
            if i >= in_bytes.len() || in_bytes[i] != fmt_bytes[j] {
                return Ok(vec![Value::Nil]);
            }
            i += 1;
            j += 1;
            continue;
        }
        j += 1;
        if j >= fmt_bytes.len() {
            return Ok(vec![Value::Nil]);
        }
        let spec = fmt_bytes[j];
        j += 1;
        match spec {
            b'Y' => match take_digits(in_bytes, &mut i, 4) {
                Some(v) => year = v as i64,
                None => return Ok(vec![Value::Nil]),
            },
            b'y' => match take_digits(in_bytes, &mut i, 2) {
                Some(v) => year = 2000 + v as i64,
                None => return Ok(vec![Value::Nil]),
            },
            b'm' => match take_digits(in_bytes, &mut i, 2) {
                Some(v) if (1..=12).contains(&v) => month = v as u32,
                _ => return Ok(vec![Value::Nil]),
            },
            b'd' => match take_digits(in_bytes, &mut i, 2) {
                Some(v) if (1..=31).contains(&v) => day = v as u32,
                _ => return Ok(vec![Value::Nil]),
            },
            b'H' => match take_digits(in_bytes, &mut i, 2) {
                Some(v) if v < 24 => hour = v as u32,
                _ => return Ok(vec![Value::Nil]),
            },
            b'M' => match take_digits(in_bytes, &mut i, 2) {
                Some(v) if v < 60 => minute = v as u32,
                _ => return Ok(vec![Value::Nil]),
            },
            b'S' => match take_digits(in_bytes, &mut i, 2) {
                Some(v) if v < 60 => second = v as u32,
                _ => return Ok(vec![Value::Nil]),
            },
            b'%' => {
                if i >= in_bytes.len() || in_bytes[i] != b'%' {
                    return Ok(vec![Value::Nil]);
                }
                i += 1;
            }
            _ => return Ok(vec![Value::Nil]),
        }
    }

    if i != in_bytes.len() {
        return Ok(vec![Value::Nil]);
    }

    let epoch = epoch_from_civil(year, month, day, hour, minute, second);
    Ok(vec![Value::Int(epoch)])
}

// ─── FsInfo / FsKind ───────────────────────────────────────────────────────

fn install_fskind_enum(env: &Rc<RefCell<Environment>>) {
    let variants = &[
        ("File", "file"),
        ("Dir", "dir"),
        ("Symlink", "symlink"),
        ("Other", "other"),
    ];
    let name = "FsKind";
    let mut variant_dict = fxmap();
    for (vname, vvalue) in variants {
        variant_dict.insert(
            (*vname).to_string(),
            Rc::new(EnumVariantObject {
                enum_name: name.to_string(),
                variant_name: (*vname).to_string(),
                value: Some(Value::Str(Rc::new((*vvalue).to_string()))),
                enum_obj: RefCell::new(None),
            }),
        );
    }
    let final_enum = Rc::new(EnumObject {
        name: name.to_string(),
        variants: variant_dict.clone(),
        tuple_variants: Default::default(),
        methods: Default::default(),
    });
    for v in variant_dict.values() {
        *v.enum_obj.borrow_mut() = Some(final_enum.clone());
    }
    env.borrow_mut()
        .define(name.to_string(), Value::Enum(final_enum));
}

/// Phantom `FsInfo` class — only used so `Value::Instance(...)` prints
/// "<instance of FsInfo>" and so the semantic registry's class name
/// lookup resolves. Has no user-callable methods of its own; field
/// access goes straight to the underlying `InstanceObject.fields` map.
fn install_fsinfo_class(env: &Rc<RefCell<Environment>>) {
    let class = Rc::new(ClassObject {
        name: "FsInfo".to_string(),
        parent: None,
        field_defs: Vec::<FieldDef>::new(),
        methods: Default::default(),
        static_fields: RefCell::new(Default::default()),
        static_methods: Default::default(),
        constructor: None,
    });
    FSINFO_CLASS.with(|slot| *slot.borrow_mut() = Some(class.clone()));
    env.borrow_mut()
        .define("FsInfo".to_string(), Value::Class(class));
}

fn fskind_variant(kind: &str) -> Value {
    // Reach into the global registry through the prelude `FsKind` name —
    // we re-look up rather than caching so unit tests that rebuild the
    // environment can't dangle a stale Rc.
    // Falls back to a freshly-constructed variant if the enum isn't
    // installed (which would only happen if the stdlib wasn't loaded).
    Value::EnumVariant(Rc::new(EnumVariantObject {
        enum_name: "FsKind".to_string(),
        variant_name: kind.to_string(),
        value: Some(Value::Str(Rc::new(kind.to_ascii_lowercase()))),
        enum_obj: RefCell::new(None),
    }))
}

/// `Os.fsInfo(path?) -> FsInfo?`
///
/// `path = nil` (or omitted) reports on the current working directory.
/// When a path is given but doesn't exist, returns `nil` so callers can
/// distinguish "missing" from "present but failed to stat" — any other
/// metadata error also collapses to `nil`.
fn os_fs_info(args: &[Value]) -> Result<Vec<Value>, String> {
    let path: String = match args.first() {
        Some(Value::Str(s)) => (**s).clone(),
        Some(Value::Nil) | None => match std::env::current_dir() {
            Ok(p) => p.to_string_lossy().into_owned(),
            Err(_) => return Ok(nil_vec()),
        },
        Some(other) => {
            return Err(format!(
                "Os.fsInfo: path must be a string or nil, got `{}`",
                other.type_name()
            ));
        }
    };

    // `symlink_metadata` so we can report `Symlink` instead of silently
    // following the link.
    let meta = match std::fs::symlink_metadata(&path) {
        Ok(m) => m,
        Err(_) => return Ok(nil_vec()),
    };

    let kind_str = if meta.file_type().is_symlink() {
        "Symlink"
    } else if meta.is_dir() {
        "Dir"
    } else if meta.is_file() {
        "File"
    } else {
        "Other"
    };

    // Not a clock read: this converts the timestamp the filesystem already
    // gave us, so it needs no `Platform` and cannot panic on wasm.
    let modified_at = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| Value::Int(d.as_secs() as i64))
        .unwrap_or(Value::Nil);

    let mut fields = fxmap();
    fields.insert("path".to_string(), Value::Str(Rc::new(path)));
    fields.insert("kind".to_string(), fskind_variant(kind_str));
    fields.insert("size".to_string(), Value::Int(meta.len() as i64));
    fields.insert("modifiedAt".to_string(), modified_at);
    fields.insert(
        "readOnly".to_string(),
        Value::Bool(meta.permissions().readonly()),
    );

    let class = FSINFO_CLASS
        .with(|slot| slot.borrow().clone())
        .ok_or_else(|| "Os.fsInfo: FsInfo class not installed".to_string())?;

    Ok(vec![Value::Instance(Rc::new(RefCell::new(
        InstanceObject { class, fields },
    )))])
}

// ─── builtin registries (consumed by saule-semantic) ───────────────────────

/// Return synthetic [`ClassInfo`] / [`EnumInfo`] entries for `Os`-owned
/// builtin types whose declarations don't exist in user source.
pub fn builtin_registries() -> (
    saule_semantic::ClassRegistry,
    saule_semantic::InterfaceRegistry,
    saule_semantic::EnumRegistry,
) {
    use saule_ast::Type;
    use saule_semantic::{ClassInfo, EnumInfo};

    let mut classes = saule_semantic::ClassRegistry::new();
    let ifaces = saule_semantic::InterfaceRegistry::new();
    let mut enums = saule_semantic::EnumRegistry::new();

    // FsKind ────────────────────────────────────────────────────────────
    let mut fskind = EnumInfo::default();
    for v in ["File", "Dir", "Symlink", "Other"] {
        fskind.variants.insert(v.to_string(), 0);
    }
    enums.insert("FsKind".to_string(), fskind);

    // FsInfo ────────────────────────────────────────────────────────────
    let mut info = ClassInfo {
        parent: None,
        implements: Vec::new(),
        members: Default::default(),
        field_types: Default::default(),
        methods: Default::default(),
    };
    let fields: [(&str, Type); 5] = [
        ("path", Type::Named("string".to_string())),
        ("kind", Type::Named("FsKind".to_string())),
        ("size", Type::Named("integer".to_string())),
        (
            "modifiedAt",
            Type::Nullable(Box::new(Type::Named("integer".to_string()))),
        ),
        ("readOnly", Type::Named("boolean".to_string())),
    ];
    for (name, ty) in fields {
        info.members.insert(name.to_string(), false);
        info.field_types.insert(name.to_string(), ty);
    }
    classes.insert("FsInfo".to_string(), info);

    (classes, ifaces, enums)
}
