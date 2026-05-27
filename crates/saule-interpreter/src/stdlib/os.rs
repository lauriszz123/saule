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

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::env::Environment;
use crate::value::{
    ClassObject, EnumObject, EnumVariantObject, FieldDef, NativeClosure, TableObject, Value,
};

// ─── installation ──────────────────────────────────────────────────────────

pub fn install(env: &Rc<RefCell<Environment>>) {
    install_platform_enum(env);

    let mut static_fields = HashMap::new();

    // time
    static_fields.insert("time".to_string(),     native_multi("Os.time",     os_time));
    static_fields.insert("clock".to_string(),    native_multi("Os.clock",    os_clock));
    static_fields.insert("difftime".to_string(), native_multi("Os.difftime", os_difftime));
    static_fields.insert("date".to_string(),     native_multi("Os.date",     os_date));
    static_fields.insert("sleep".to_string(),    native_multi("Os.sleep",    os_sleep));

    // environment
    static_fields.insert("getenv".to_string(),   native_multi("Os.getenv",   os_getenv));
    static_fields.insert("setenv".to_string(),   native_multi("Os.setenv",   os_setenv));
    static_fields.insert("cwd".to_string(),      native_multi("Os.cwd",      os_cwd));
    static_fields.insert("chdir".to_string(),    native_multi("Os.chdir",    os_chdir));

    // filesystem
    static_fields.insert("remove".to_string(),   native_multi("Os.remove",   os_remove));
    static_fields.insert("rename".to_string(),   native_multi("Os.rename",   os_rename));
    static_fields.insert("list".to_string(),     native_multi("Os.list",     os_list));
    static_fields.insert("exists".to_string(),   native_multi("Os.exists",   os_exists));
    static_fields.insert("mkdir".to_string(),    native_multi("Os.mkdir",    os_mkdir));
    static_fields.insert("tmpname".to_string(),  native_multi("Os.tmpname",  os_tmpname));

    // process
    static_fields.insert("exit".to_string(),     native_multi("Os.exit",     os_exit));
    static_fields.insert("execute".to_string(),  native_multi("Os.execute",  os_execute));
    static_fields.insert("pid".to_string(),      native_multi("Os.pid",      os_pid));
    static_fields.insert("platform".to_string(), native_multi("Os.platform", os_platform));
    static_fields.insert("args".to_string(),     native_multi("Os.args",     os_args));

    // constants
    static_fields.insert("sep".to_string(),     Value::Str(Rc::new(path_sep().to_string())));
    static_fields.insert("lineSep".to_string(), Value::Str(Rc::new(line_sep().to_string())));

    let class = ClassObject {
        name: "Os".to_string(),
        parent: None,
        field_defs: Vec::<FieldDef>::new(),
        methods: HashMap::new(),
        static_fields: RefCell::new(static_fields),
        static_methods: HashMap::new(),
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
    let table_str = || Type::Table { key: None, value: Box::new(s()) };

    // time
    register("Os.time",     vec![],                       vec![i()]);
    register("Os.clock",    vec![],                       vec![f()]);
    register("Os.difftime", vec![i(), i()],               vec![i()]);
    register("Os.date",     vec![t_nullable(s()), t_nullable(i())], vec![s()]);
    register("Os.sleep",    vec![t_number()],             vec![nil()]);

    // environment
    register("Os.getenv",   vec![s()],                    vec![str_opt()]);
    register("Os.setenv",   vec![s(), s()],               vec![nil()]);
    register("Os.cwd",      vec![],                       vec![s()]);
    register("Os.chdir",    vec![s()],                    vec![b()]);

    // filesystem
    register("Os.remove",   vec![s()],                    vec![b()]);
    register("Os.rename",   vec![s(), s()],               vec![b()]);
    register("Os.list",     vec![s()],                    vec![table_str()]);
    register("Os.exists",   vec![s()],                    vec![b()]);
    register("Os.mkdir",    vec![s(), t_nullable(b())],   vec![b()]);
    register("Os.tmpname",  vec![],                       vec![s()]);

    // process
    register("Os.exit",     vec![t_nullable(i())],        vec![nil()]);
    register("Os.execute",  vec![s()],                    vec![i()]);
    register("Os.pid",      vec![],                       vec![i()]);
    register("Os.platform", vec![],                       vec![t_named("OsPlatform")]);
    register("Os.args",     vec![],                       vec![table_str()]);
}

// ─── enum ──────────────────────────────────────────────────────────────────

fn install_platform_enum(env: &Rc<RefCell<Environment>>) {
    let variants = &[
        ("Linux",   "linux"),
        ("Macos",   "macos"),
        ("Windows", "windows"),
        ("Other",   "other"),
    ];
    let name = "OsPlatform";
    let mut variant_dict = HashMap::new();
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
        tuple_variants: HashMap::new(),
        methods: HashMap::new(),
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
    static START_INSTANT: Instant = Instant::now();
}

/// Called from the CLI before running user code to publish argv to `Os.args()`.
pub fn set_script_args(args: Vec<String>) {
    SCRIPT_ARGS.with(|cell| *cell.borrow_mut() = args);
}

// ─── native helpers ────────────────────────────────────────────────────────

fn native_multi(
    name: &'static str,
    func: fn(&[Value]) -> Result<Vec<Value>, String>,
) -> Value {
    Value::NativeClosure(Rc::new(NativeClosure {
        name,
        func: Box::new(move |args| func(args)),
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

#[cfg(target_os = "linux")]   fn platform_str() -> &'static str { "linux" }
#[cfg(target_os = "macos")]   fn platform_str() -> &'static str { "macos" }
#[cfg(target_os = "windows")] fn platform_str() -> &'static str { "windows" }
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn platform_str() -> &'static str { "other" }

#[cfg(target_family = "windows")]
fn path_sep() -> &'static str { "\\" }
#[cfg(not(target_family = "windows"))]
fn path_sep() -> &'static str { "/" }

#[cfg(target_family = "windows")]
fn line_sep() -> &'static str { "\r\n" }
#[cfg(not(target_family = "windows"))]
fn line_sep() -> &'static str { "\n" }

// ─── time ──────────────────────────────────────────────────────────────────

fn os_time(_args: &[Value]) -> Result<Vec<Value>, String> {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Ok(vec![Value::Int(secs)])
}

fn os_clock(_args: &[Value]) -> Result<Vec<Value>, String> {
    let elapsed = START_INSTANT.with(|i| i.elapsed());
    Ok(vec![Value::Float(elapsed.as_secs_f64())])
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
        Some(Value::Nil) | None => SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
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
    if secs > 0.0 && secs.is_finite() {
        std::thread::sleep(std::time::Duration::from_secs_f64(secs));
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
    unsafe { std::env::set_var(name, value); }
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
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let path = dir.join(format!("saule_{pid}_{nanos}.tmp"));
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
    std::process::exit(code);
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
    Ok(vec![Value::Int(std::process::id() as i64)])
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
    let argv: Vec<Value> = SCRIPT_ARGS.with(|cell| {
        cell.borrow().iter().map(|s| str_value(s.clone())).collect()
    });
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
            Some('c') => out.push_str(&format!(
                "{y:04}-{mo:02}-{d:02} {hh:02}:{mm:02}:{ss:02}"
            )),
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
    let rem  = epoch.rem_euclid(86_400) as u32;
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
    let year = if m_shifted <= 2 { y_shifted + 1 } else { y_shifted };

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

