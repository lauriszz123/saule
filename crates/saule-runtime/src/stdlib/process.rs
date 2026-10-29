//! `Process` — the methods on the handle `Os.spawn` returns.
//!
//! Laid out like `stdlib::io`'s `File`: the methods are dispatched off a
//! `Value::Process` by [`dispatch_process_method`] rather than living on a
//! class, and their signatures are registered for the typechecker under the
//! module name `Process` so `p.read()` checks and propagates its type.

use std::cell::RefCell;
use std::rc::Rc;

use crate::value::{ProcessHandle, SauleBytes, Value};

/// How much one `read` takes when the caller names no limit.
///
/// A cap rather than "everything": a child that outruns its reader would
/// otherwise hand back a buffer that grows without bound, and a frame loop
/// wants a predictable slice of work per call. 64 KiB is a pipe buffer's
/// worth, so a well-behaved reader keeps up with one call per frame.
const DEFAULT_READ: usize = 64 * 1024;

/// Route `process.METHOD(args...)` calls.
pub fn dispatch_process_method(
    handle: &Rc<RefCell<ProcessHandle>>,
    method: &str,
    args: &[Value],
) -> Result<Vec<Value>, String> {
    match method {
        "read" => read_stream(handle, args, Which::Out),
        "readErr" => read_stream(handle, args, Which::Err),
        "write" => write(handle, args),
        "closeStdin" => {
            handle.borrow_mut().close_stdin();
            Ok(vec![Value::Nil])
        }
        "poll" => match handle.borrow_mut().poll()? {
            Some(code) => Ok(vec![Value::Int(code)]),
            None => Ok(vec![Value::Nil]),
        },
        "wait" => {
            let code = handle.borrow_mut().wait()?;
            Ok(vec![Value::Int(code)])
        }
        "kill" => {
            let killed = handle.borrow_mut().kill();
            Ok(vec![Value::Bool(killed)])
        }
        "running" => {
            let running = handle.borrow_mut().running();
            Ok(vec![Value::Bool(running)])
        }
        "pid" => {
            let pid = handle.borrow().pid() as i64;
            Ok(vec![Value::Int(pid)])
        }
        "available" => {
            let n = handle.borrow().stdout().available() as i64;
            Ok(vec![Value::Int(n)])
        }
        "atEof" => {
            let eof = handle.borrow().stdout().at_eof();
            Ok(vec![Value::Bool(eof)])
        }
        other => Err(format!(
            "no method `{other}` on process — valid: read / readErr / write / \
             closeStdin / poll / wait / kill / running / pid / available / atEof"
        )),
    }
}

/// Which captured stream a read is about.
enum Which {
    Out,
    Err,
}

fn read_stream(
    handle: &Rc<RefCell<ProcessHandle>>,
    args: &[Value],
    which: Which,
) -> Result<Vec<Value>, String> {
    let name = match which {
        Which::Out => "Process.read",
        Which::Err => "Process.readErr",
    };
    // A negative or zero limit reads nothing rather than erroring: it is
    // what a computed `max` of `budget - used` naturally reaches.
    let max = match expect_int_opt(name, args, 0)? {
        Some(n) if n <= 0 => return Ok(vec![Value::Bytes(SauleBytes::new(Vec::new()))]),
        Some(n) => n as usize,
        None => DEFAULT_READ,
    };
    let handle = handle.borrow();
    let stream = match which {
        Which::Out => handle.stdout(),
        Which::Err => handle.stderr(),
    };
    Ok(vec![Value::Bytes(SauleBytes::new(stream.take(max)))])
}

fn write(handle: &Rc<RefCell<ProcessHandle>>, args: &[Value]) -> Result<Vec<Value>, String> {
    let data = expect_bytes("Process.write", args, 0)?;
    let n = handle.borrow_mut().write(&data)?;
    Ok(vec![Value::Int(n as i64)])
}

fn expect_bytes(name: &str, args: &[Value], idx: usize) -> Result<SauleBytes, String> {
    match args.get(idx) {
        Some(Value::Bytes(b)) => Ok(b.clone()),
        Some(other) => Err(format!(
            "{name} expects bytes at argument {}, got `{}`",
            idx + 1,
            other.type_name()
        )),
        None => Err(format!("{name} missing argument {}", idx + 1)),
    }
}

/// An optional integer argument. `nil` and an absent argument mean the same
/// thing, which is what lets `p.read()` and `p.read(nil)` agree.
fn expect_int_opt(name: &str, args: &[Value], idx: usize) -> Result<Option<i64>, String> {
    match args.get(idx) {
        None | Some(Value::Nil) => Ok(None),
        Some(Value::Int(n)) => Ok(Some(*n)),
        Some(other) => Err(format!(
            "{name} expects an integer at argument {}, got `{}`",
            idx + 1,
            other.type_name()
        )),
    }
}

/// Register native signatures for the typechecker.
///
/// Called from `stdlib::os`'s own `register_sigs`, since `Os.spawn` is what
/// produces a `Process` and the two have to agree on the name.
pub fn register_sigs() {
    use crate::stdlib::sigs::{register, register_module, t_named, t_nullable};
    let b = || t_named("bytes");
    let i = || t_named("integer");
    let int_opt = || t_nullable(i());
    let nil = || t_named("nil");

    // As with `File`: registered as a module so a static-call typo surfaces
    // at typeck time, then one signature per instance method.
    register_module("Process");
    // `p.read(max?: integer) -> bytes` — what has arrived, never blocking.
    // Typed `bytes`, not `bytes?`: "nothing yet" is the empty buffer, and a
    // nullable return would make every caller unwrap a non-event.
    register("Process.read", vec![int_opt()], vec![b()]);
    register("Process.readErr", vec![int_opt()], vec![b()]);
    // `p.write(data: bytes) -> integer`
    register("Process.write", vec![b()], vec![i()]);
    // `p.closeStdin() -> nil`
    register("Process.closeStdin", vec![], vec![nil()]);
    // `p.poll() -> integer?` — the exit code, or nil while it runs.
    register("Process.poll", vec![], vec![int_opt()]);
    // `p.wait() -> integer`
    register("Process.wait", vec![], vec![i()]);
    // `p.kill() -> boolean`
    register("Process.kill", vec![], vec![t_named("boolean")]);
    // `p.running() -> boolean`
    register("Process.running", vec![], vec![t_named("boolean")]);
    // `p.pid() -> integer`
    register("Process.pid", vec![], vec![i()]);
    // `p.available() -> integer`
    register("Process.available", vec![], vec![i()]);
    // `p.atEof() -> boolean`
    register("Process.atEof", vec![], vec![t_named("boolean")]);
}
