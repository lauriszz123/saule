//! `Io` static class + `File` value type + `IoMode`/`IoSeek` enums.
//!
//! Design choices:
//!
//! * `Io` is a static class (like `Math` / `String` / `Table`) with statics
//!   `stdin` / `stdout` / `stderr` and methods `open` / `lines` / `read` /
//!   `write`.
//!
//! * File handles are first-class `Value::File` values. Their methods —
//!   `read` / `write` / `lines` / `seek` / `flush` / `close` — are dispatched
//!   by the evaluator via [`dispatch_file_method`] rather than living on a
//!   regular `ClassObject`. A phantom `File` class is still registered so the
//!   typechecker recognises `File` as a type name.
//!
//! * Mode and seek-whence are real Saule **enums** (`IoMode`, `IoSeek`),
//!   registered programmatically. Natives read the variant's `.value` string
//!   to drive the underlying `OpenOptions` / `SeekFrom`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::rc::Rc;

use crate::env::Environment;
use crate::value::{
    ClassObject, EnumObject, EnumVariantObject, FieldDef, FileHandle, NativeClosure, Value,
};

// ─── installation ──────────────────────────────────────────────────────────

pub fn install(env: &Rc<RefCell<Environment>>) {
    install_enum(
        env,
        "IoMode",
        &[
            ("Read", "r"),
            ("Write", "w"),
            ("Append", "a"),
            ("ReadWrite", "r+"),
            ("WriteRead", "w+"),
            ("AppendRead", "a+"),
            ("ReadBinary", "rb"),
            ("WriteBinary", "wb"),
            ("AppendBinary", "ab"),
        ],
    );
    install_enum(
        env,
        "IoSeek",
        &[("Set", "set"), ("Cur", "cur"), ("End", "end")],
    );

    // Phantom `File` class — only needed so the typechecker recognises
    // `File` as a type name in `local f: File = ...`. Method dispatch happens
    // in `dispatch_file_method`, not through this class.
    let file_class = ClassObject {
        name: "File".to_string(),
        parent: None,
        field_defs: Vec::<FieldDef>::new(),
        methods: HashMap::new(),
        static_fields: RefCell::new(HashMap::new()),
        static_methods: HashMap::new(),
        constructor: None,
    };
    env.borrow_mut()
        .define("File".to_string(), Value::Class(Rc::new(file_class)));

    // `Io` class — statics + native methods.
    let mut static_fields = HashMap::new();
    static_fields.insert(
        "stdin".to_string(),
        Value::File(Rc::new(RefCell::new(FileHandle::Stdin(BufReader::new(
            std::io::stdin(),
        ))))),
    );
    static_fields.insert(
        "stdout".to_string(),
        Value::File(Rc::new(RefCell::new(FileHandle::Stdout))),
    );
    static_fields.insert(
        "stderr".to_string(),
        Value::File(Rc::new(RefCell::new(FileHandle::Stderr))),
    );
    static_fields.insert("open".to_string(), native_multi("Io.open", io_open));
    static_fields.insert("lines".to_string(), native_multi("Io.lines", io_lines));
    static_fields.insert("read".to_string(), native_multi("Io.read", io_read));
    static_fields.insert("write".to_string(), native_multi("Io.write", io_write));

    let io_class = ClassObject {
        name: "Io".to_string(),
        parent: None,
        field_defs: Vec::new(),
        methods: HashMap::new(),
        static_fields: RefCell::new(static_fields),
        static_methods: HashMap::new(),
        constructor: None,
    };
    env.borrow_mut()
        .define("Io".to_string(), Value::Class(Rc::new(io_class)));
}

/// Register native signatures for the typechecker (lazy, via `sigs::lookup`).
pub fn register_sigs() {
    use crate::stdlib::sigs::{register, register_v, t_function, t_named, t_nullable};
    let s = || t_named("string");
    let nil = || t_named("nil");
    let file_opt = || t_nullable(t_named("File"));
    let file = || t_named("File");
    let str_opt = || t_nullable(s());
    // `fn(): string?` — the shape returned by `Io.lines` / `File.lines`,
    // suitable for `for line in Io.lines(...) do ... end`.
    let line_iter = || t_function(vec![], t_nullable(s()));

    register("Io.open", vec![s(), t_named("IoMode")], vec![file_opt()]);
    register("Io.lines", vec![t_nullable(s())], vec![line_iter()]);
    // `Io.read(...formats: string) -> string?` — zero-or-more strings.
    register_v("Io.read", vec![], s(), vec![str_opt()]);
    // `Io.write(...parts: string) -> nil` — zero-or-more strings.
    register_v("Io.write", vec![], s(), vec![nil()]);

    // `File` is a *value* class — its methods (`read`, `write`, `lines`,
    // `seek`, `flush`, `close`) are dispatched off a `Value::File`
    // instance, not off the class itself. Register `File` as a module
    // so static-call typos like `File.write(self.path, data)` surface
    // as `UnknownMember` at typeck time, then register a sig for each
    // instance method so the typechecker validates argument types and
    // propagates return types for `file.method(...)` calls.
    use crate::stdlib::sigs::register_module;
    register_module("File");
    let int_opt = || t_nullable(t_named("integer"));
    // `file.read(format?: string) -> string?`
    register("File.read", vec![t_nullable(s())], vec![str_opt()]);
    // `file.write(...parts: string) -> nil`
    register_v("File.write", vec![], s(), vec![nil()]);
    // `file.lines() -> fn(): string?`
    register("File.lines", vec![], vec![line_iter()]);
    // `file.seek(whence?: IoSeek, offset?: integer) -> integer?`
    register(
        "File.seek",
        vec![t_nullable(t_named("IoSeek")), int_opt()],
        vec![int_opt()],
    );
    // `file.flush() -> nil`
    register("File.flush", vec![], vec![nil()]);
    // `file.close() -> nil`
    register("File.close", vec![], vec![nil()]);
    let _ = file;

    // `File`-valued constants on the `Io` static class.
    use crate::stdlib::sigs::register_member;
    register_member("Io.stdin");
    register_member("Io.stdout");
    register_member("Io.stderr");
}

// ─── enum helper ───────────────────────────────────────────────────────────

fn install_enum(env: &Rc<RefCell<Environment>>, name: &str, variants: &[(&str, &str)]) {
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

// ─── native helpers ────────────────────────────────────────────────────────

fn native_multi(name: &'static str, func: fn(&[Value]) -> Result<Vec<Value>, String>) -> Value {
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

/// Extract the underlying string value from an `IoMode` / `IoSeek` variant,
/// or accept a raw string (handy for tests and ad-hoc calls).
fn extract_enum_string(name: &str, args: &[Value], idx: usize) -> Result<String, String> {
    match args.get(idx) {
        Some(Value::EnumVariant(ev)) => match &ev.value {
            Some(Value::Str(s)) => Ok((**s).clone()),
            _ => Err(format!(
                "{name}: enum variant `{}.{}` has no string value",
                ev.enum_name, ev.variant_name
            )),
        },
        Some(Value::Str(s)) => Ok((**s).clone()),
        Some(other) => Err(format!(
            "{name} expects an enum variant or string at argument {}, got `{}`",
            idx + 1,
            other.type_name()
        )),
        None => Err(format!("{name} missing argument {}", idx + 1)),
    }
}

// ─── Io.open ─────────────────────────���─────────────────────────────────────

fn io_open(args: &[Value]) -> Result<Vec<Value>, String> {
    let path = expect_string("Io.open", args, 0)?;
    let mode = if args.len() >= 2 {
        extract_enum_string("Io.open", args, 1)?
    } else {
        "r".to_string()
    };

    let mut opts = OpenOptions::new();
    let (readable, writable) = match mode.as_str() {
        "r" | "rb" => {
            opts.read(true);
            (true, false)
        }
        "w" | "wb" => {
            opts.write(true).create(true).truncate(true);
            (false, true)
        }
        "a" | "ab" => {
            opts.append(true).create(true);
            (false, true)
        }
        "r+" | "rb+" | "r+b" => {
            opts.read(true).write(true);
            (true, true)
        }
        "w+" | "wb+" | "w+b" => {
            opts.read(true).write(true).create(true).truncate(true);
            (true, true)
        }
        "a+" | "ab+" | "a+b" => {
            opts.read(true).append(true).create(true);
            (true, true)
        }
        other => return Err(format!("Io.open: unknown mode `{other}`")),
    };

    match opts.open(&path) {
        Ok(file) => {
            let reader = if readable {
                let cloned = file.try_clone().map_err(|e| e.to_string())?;
                Some(BufReader::new(cloned))
            } else {
                None
            };
            let writer = if writable { Some(file) } else { None };
            Ok(vec![Value::File(Rc::new(RefCell::new(FileHandle::Open {
                path,
                reader,
                writer,
            })))])
        }
        // Lua semantics: failure returns nil (we drop the OS errno from the
        // tuple — `Result<File>` would be the richer alternative).
        Err(_) => Ok(vec![Value::Nil]),
    }
}

// ─── Io.lines ──────────────────────────────────────────────────────────────

fn io_lines(args: &[Value]) -> Result<Vec<Value>, String> {
    let path = match args.first() {
        Some(Value::Str(s)) => (**s).clone(),
        Some(Value::Nil) | None => {
            // No path → iterate stdin.
            let reader = Rc::new(RefCell::new(BufReader::new(std::io::stdin())));
            return Ok(vec![lines_step_stdin(reader)]);
        }
        Some(other) => {
            return Err(format!(
                "Io.lines expects a string path or nil, got `{}`",
                other.type_name()
            ));
        }
    };

    let file = std::fs::File::open(&path).map_err(|e| format!("Io.lines: {e}"))?;
    let reader = Rc::new(RefCell::new(BufReader::new(file)));
    Ok(vec![lines_step_file(reader)])
}

fn lines_step_file(reader: Rc<RefCell<BufReader<std::fs::File>>>) -> Value {
    Value::NativeClosure(Rc::new(NativeClosure {
        name: "Io.lines#step",
        func: Box::new(move |_| {
            let mut buf = String::new();
            let mut r = reader.borrow_mut();
            match r.read_line(&mut buf) {
                Ok(0) => Ok(vec![Value::Nil]),
                Ok(_) => {
                    strip_trailing_newline(&mut buf);
                    Ok(vec![Value::Str(Rc::new(buf))])
                }
                Err(e) => Err(format!("Io.lines: read error: {e}")),
            }
        }),
    }))
}

fn lines_step_stdin(reader: Rc<RefCell<BufReader<std::io::Stdin>>>) -> Value {
    Value::NativeClosure(Rc::new(NativeClosure {
        name: "Io.lines#stdin-step",
        func: Box::new(move |_| {
            let mut buf = String::new();
            let mut r = reader.borrow_mut();
            match r.read_line(&mut buf) {
                Ok(0) => Ok(vec![Value::Nil]),
                Ok(_) => {
                    strip_trailing_newline(&mut buf);
                    Ok(vec![Value::Str(Rc::new(buf))])
                }
                Err(e) => Err(format!("Io.lines: read error: {e}")),
            }
        }),
    }))
}

fn strip_trailing_newline(s: &mut String) {
    if s.ends_with('\n') {
        s.pop();
        if s.ends_with('\r') {
            s.pop();
        }
    }
}

// ─── Io.read / Io.write — operate on stdin / stdout ────────────────────────

fn io_read(args: &[Value]) -> Result<Vec<Value>, String> {
    let format = if args.is_empty() {
        "l".to_string()
    } else {
        expect_string("Io.read", args, 0)?
    };
    read_format(&format, &mut ReadSource::Stdin)
}

fn io_write(args: &[Value]) -> Result<Vec<Value>, String> {
    let mut out = std::io::stdout();
    for v in args {
        let _ = out.write_all(v.to_display_string().as_bytes());
    }
    let _ = out.flush();
    Ok(vec![Value::Nil])
}

// ─── shared read format ────────────────────────────────────────────────────

enum ReadSource<'a> {
    Stdin,
    BufFile(&'a mut BufReader<std::fs::File>),
    BufStdin(&'a mut BufReader<std::io::Stdin>),
}

fn read_format(fmt: &str, src: &mut ReadSource<'_>) -> Result<Vec<Value>, String> {
    // Lua-style format characters: "l" line w/o newline, "L" line w/ newline,
    // "a" everything, "n" integer (best-effort).
    let fmt = fmt.trim_start_matches('*');
    match fmt {
        "l" | "L" => {
            let mut buf = String::new();
            let bytes = read_line_src(src, &mut buf)?;
            if bytes == 0 {
                return Ok(vec![Value::Nil]);
            }
            if fmt == "l" {
                strip_trailing_newline(&mut buf);
            }
            Ok(vec![Value::Str(Rc::new(buf))])
        }
        "a" => {
            let mut buf = String::new();
            read_all_src(src, &mut buf)?;
            Ok(vec![Value::Str(Rc::new(buf))])
        }
        "n" => {
            let mut buf = String::new();
            let bytes = read_line_src(src, &mut buf)?;
            if bytes == 0 {
                return Ok(vec![Value::Nil]);
            }
            match buf.trim().parse::<i64>() {
                Ok(n) => Ok(vec![Value::Int(n)]),
                Err(_) => Ok(vec![Value::Nil]),
            }
        }
        other => Err(format!("Io.read: unsupported format `{other}`")),
    }
}

fn read_line_src(src: &mut ReadSource<'_>, buf: &mut String) -> Result<usize, String> {
    let r = match src {
        ReadSource::Stdin => {
            let stdin = std::io::stdin();
            return stdin.lock().read_line(buf).map_err(|e| e.to_string());
        }
        ReadSource::BufFile(r) => r.read_line(buf),
        ReadSource::BufStdin(r) => r.read_line(buf),
    };
    r.map_err(|e| e.to_string())
}

fn read_all_src(src: &mut ReadSource<'_>, buf: &mut String) -> Result<(), String> {
    match src {
        ReadSource::Stdin => {
            std::io::stdin()
                .lock()
                .read_to_string(buf)
                .map_err(|e| e.to_string())?;
        }
        ReadSource::BufFile(r) => {
            r.read_to_string(buf).map_err(|e| e.to_string())?;
        }
        ReadSource::BufStdin(r) => {
            r.read_to_string(buf).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

// ─── file method dispatch ──────────────────────────────────────────────────

/// Route `file.METHOD(args...)` calls. Called from `dispatch_member_call_multi`.
pub fn dispatch_file_method(
    handle: &Rc<RefCell<FileHandle>>,
    method: &str,
    args: &[Value],
) -> Result<Vec<Value>, String> {
    match method {
        "read" => file_read(handle, args),
        "write" => file_write(handle, args),
        "lines" => file_lines(handle, args),
        "seek" => file_seek(handle, args),
        "flush" => file_flush(handle, args),
        "close" => file_close(handle, args),
        other => Err(format!(
            "no method `{other}` on file — valid: read / write / lines / seek / flush / close"
        )),
    }
}

fn require_open<'a>(
    handle: &'a mut std::cell::RefMut<'_, FileHandle>,
) -> Result<&'a mut FileHandle, String> {
    if matches!(**handle, FileHandle::Closed) {
        return Err("file is closed".to_string());
    }
    Ok(&mut **handle)
}

fn file_read(handle: &Rc<RefCell<FileHandle>>, args: &[Value]) -> Result<Vec<Value>, String> {
    let format = if args.is_empty() {
        "l".to_string()
    } else {
        expect_string("File.read", args, 0)?
    };
    let mut h = handle.borrow_mut();
    let h = require_open(&mut h)?;
    match h {
        FileHandle::Open {
            reader: Some(r), ..
        } => {
            let mut src = ReadSource::BufFile(r);
            read_format(&format, &mut src)
        }
        FileHandle::Stdin(r) => {
            let mut src = ReadSource::BufStdin(r);
            read_format(&format, &mut src)
        }
        FileHandle::Open { reader: None, .. } => Err("file is not opened for reading".to_string()),
        FileHandle::Stdout | FileHandle::Stderr => {
            Err("cannot read from stdout/stderr".to_string())
        }
        FileHandle::Closed => unreachable!(),
    }
}

fn file_write(handle: &Rc<RefCell<FileHandle>>, args: &[Value]) -> Result<Vec<Value>, String> {
    let mut h = handle.borrow_mut();
    let h = require_open(&mut h)?;
    match h {
        FileHandle::Open {
            writer: Some(w), ..
        } => {
            for v in args {
                w.write_all(v.to_display_string().as_bytes())
                    .map_err(|e| e.to_string())?;
            }
            Ok(vec![Value::Nil])
        }
        FileHandle::Stdout => {
            let mut out = std::io::stdout();
            for v in args {
                let _ = out.write_all(v.to_display_string().as_bytes());
            }
            let _ = out.flush();
            Ok(vec![Value::Nil])
        }
        FileHandle::Stderr => {
            let mut err = std::io::stderr();
            for v in args {
                let _ = err.write_all(v.to_display_string().as_bytes());
            }
            let _ = err.flush();
            Ok(vec![Value::Nil])
        }
        FileHandle::Open { writer: None, .. } => Err("file is not opened for writing".to_string()),
        FileHandle::Stdin(_) => Err("cannot write to stdin".to_string()),
        FileHandle::Closed => unreachable!(),
    }
}

fn file_lines(handle: &Rc<RefCell<FileHandle>>, _args: &[Value]) -> Result<Vec<Value>, String> {
    // Build a step closure that re-borrows the same handle each iteration.
    let h = handle.clone();
    Ok(vec![Value::NativeClosure(Rc::new(NativeClosure {
        name: "File.lines#step",
        func: Box::new(move |_| {
            let mut handle = h.borrow_mut();
            match &mut *handle {
                FileHandle::Open {
                    reader: Some(r), ..
                } => {
                    let mut buf = String::new();
                    match r.read_line(&mut buf) {
                        Ok(0) => Ok(vec![Value::Nil]),
                        Ok(_) => {
                            strip_trailing_newline(&mut buf);
                            Ok(vec![Value::Str(Rc::new(buf))])
                        }
                        Err(e) => Err(format!("File.lines: {e}")),
                    }
                }
                FileHandle::Stdin(r) => {
                    let mut buf = String::new();
                    match r.read_line(&mut buf) {
                        Ok(0) => Ok(vec![Value::Nil]),
                        Ok(_) => {
                            strip_trailing_newline(&mut buf);
                            Ok(vec![Value::Str(Rc::new(buf))])
                        }
                        Err(e) => Err(format!("File.lines: {e}")),
                    }
                }
                FileHandle::Closed => Err("File.lines: file is closed".to_string()),
                _ => Err("File.lines: file is not readable".to_string()),
            }
        }),
    }))])
}

fn file_seek(handle: &Rc<RefCell<FileHandle>>, args: &[Value]) -> Result<Vec<Value>, String> {
    let whence = if args.is_empty() {
        "cur".to_string()
    } else {
        extract_enum_string("File.seek", args, 0)?
    };
    let offset = if args.len() >= 2 {
        match &args[1] {
            Value::Int(n) => *n,
            other => {
                return Err(format!(
                    "File.seek expects an integer offset, got `{}`",
                    other.type_name()
                ));
            }
        }
    } else {
        0
    };

    let from = match whence.as_str() {
        "set" => SeekFrom::Start(offset.max(0) as u64),
        "cur" => SeekFrom::Current(offset),
        "end" => SeekFrom::End(offset),
        other => return Err(format!("File.seek: invalid whence `{other}`")),
    };
    let mut h = handle.borrow_mut();
    let h = require_open(&mut h)?;
    match h {
        FileHandle::Open { reader, writer, .. } => {
            // Prefer seeking the writer if present (matches Lua's behaviour
            // on r+/w+/a+: the file shares the OS cursor).
            let pos = if let Some(w) = writer {
                w.seek(from).map_err(|e| e.to_string())?
            } else if let Some(r) = reader {
                // BufReader seeks its inner File, then reseeks its buffer.
                r.seek(from).map_err(|e| e.to_string())?
            } else {
                return Err("file has no seekable backing".to_string());
            };
            Ok(vec![Value::Int(pos as i64)])
        }
        _ => Err("File.seek: not supported on standard streams".to_string()),
    }
}

fn file_flush(handle: &Rc<RefCell<FileHandle>>, _args: &[Value]) -> Result<Vec<Value>, String> {
    let mut h = handle.borrow_mut();
    let h = require_open(&mut h)?;
    match h {
        FileHandle::Open {
            writer: Some(w), ..
        } => {
            w.flush().map_err(|e| e.to_string())?;
            Ok(vec![Value::Nil])
        }
        FileHandle::Stdout => {
            let _ = std::io::stdout().flush();
            Ok(vec![Value::Nil])
        }
        FileHandle::Stderr => {
            let _ = std::io::stderr().flush();
            Ok(vec![Value::Nil])
        }
        _ => Ok(vec![Value::Nil]),
    }
}

fn file_close(handle: &Rc<RefCell<FileHandle>>, _args: &[Value]) -> Result<Vec<Value>, String> {
    let mut h = handle.borrow_mut();
    // Standard streams are no-ops on close to match Lua.
    if matches!(
        *h,
        FileHandle::Stdout | FileHandle::Stderr | FileHandle::Stdin(_)
    ) {
        return Ok(vec![Value::Nil]);
    }
    *h = FileHandle::Closed;
    Ok(vec![Value::Nil])
}
