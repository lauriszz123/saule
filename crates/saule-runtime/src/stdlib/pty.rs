//! `Pty` — a program run behind a pseudo-terminal: what a terminal emulator
//! is built on.
//!
//! The program sees a real terminal: it can ask for the window size, switch
//! the terminal to raw mode, get Ctrl-C as a signal and do job control. The
//! Saule program on the other side reads everything it writes — escape codes
//! and all, for the terminal emulator to interpret — and writes the
//! keystrokes it should receive.
//!
//! ```saule
//! local sh = Os.spawnPty("/bin/sh")!  -- nil if it cannot start
//! sh.resize(80, 24)                    -- columns, rows; the shell is told
//! sh.write(Bytes.fromString("ls\n"))   -- keystrokes in
//! local out: bytes = sh.read()         -- what arrived; empty if nothing yet
//! sh.poll()                            -- the exit code, nil while it runs
//! ```
//!
//! Methods:
//!   * `Os.spawnPty(program: string, args: table<string>?) -> Pty?` — start
//!     `program` with `args` on a new 80×24 terminal. The environment is
//!     inherited: set `TERM` to what your emulator understands first.
//!   * `p.read(max: integer?) -> bytes` — what the program has written,
//!     never blocking; empty when there is nothing yet.
//!   * `p.write(data: bytes) -> integer` — keystrokes for the program; how
//!     many bytes were taken.
//!   * `p.resize(cols: integer, rows: integer)` — the terminal's new size;
//!     the program is told (`SIGWINCH` on Unix).
//!   * `p.poll() -> integer?`, `p.running() -> boolean`, `p.pid() ->
//!     integer`, `p.kill() -> boolean`, `p.atEof() -> boolean` — as on
//!     `Process`. `atEof` is true once the program has gone and everything
//!     it wrote has been read.
//!   * `p.close()` — hang up: the program gets `SIGHUP`, as when a terminal
//!     window closes.
//!
//! Unix: `openpty`, and the program gets the terminal as its controlling
//! terminal in a session of its own. Windows: ConPTY (`CreatePseudoConsole`),
//! what Windows Terminal is built on.

use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;

use crate::fxhash::fxmap;
use crate::native_packages::NativePackage;
use crate::prelude::Prelude;
use crate::value::{ForeignClass, ForeignObject, NativeClosure, SauleBytes, Value};

/// Only signatures: `Pty` values come from `Os.spawnPty` (installed with
/// `Os`, as `Process` values come from `Os.spawn`), and a value type has no
/// statics of its own.
pub static PTY_PACKAGE: NativePackage = NativePackage {
    name: "pty",
    version: saule_version::VERSION,
    install,
    exports: &[],
    register_sigs,
    builtins: empty_builtins,
    auto_prelude: true,
};

fn empty_builtins() -> saule_semantic::builtins::Builtins {
    saule_semantic::builtins::Builtins::default()
}

/// The size a new terminal starts at, until the first `resize`.
const START_COLS: u16 = 80;
const START_ROWS: u16 = 24;

/// What a `Pty` value's pointer points at.
type Shared = RefCell<sys::Term>;

const PACKAGE: &str = "#pty";

fn native(name: &'static str, func: impl Fn(&[Value]) -> Result<Vec<Value>, String> + 'static) -> Value {
    Value::NativeClosure(Rc::new(NativeClosure {
        name,
        func: Box::new(func),
        param_names: Vec::new(),
    }))
}

pub fn install(_env: &Rc<RefCell<Prelude>>) {}

/// The class a `Pty` value's methods are found on.
fn pty_class() -> Rc<ForeignClass> {
    thread_local! {
        static CLASS: Rc<ForeignClass> = {
            let mut methods = fxmap();
            methods.insert("read".to_string(), native("Pty.read", |a| with_term(a, "Pty.read", |t| {
                let max = match a.get(1) {
                    None | Some(Value::Nil) => usize::MAX,
                    Some(Value::Int(n)) if *n >= 0 => *n as usize,
                    Some(other) => return Err(super::wrong_arg("Pty.read", 0, "a non-negative `integer` or `nil`", other)),
                };
                Ok(vec![Value::Bytes(SauleBytes::new(t.read(max)))])
            })));
            methods.insert("write".to_string(), native("Pty.write", |a| with_term(a, "Pty.write", |t| {
                let data = match a.get(1) {
                    Some(Value::Bytes(b)) => b.as_slice().to_vec(),
                    Some(Value::Str(s)) => s.as_str().as_bytes().to_vec(),
                    Some(other) => return Err(super::wrong_arg("Pty.write", 0, "`bytes`", other)),
                    None => return Err(super::missing_arg("Pty.write", 0)),
                };
                let n = t.write(&data).map_err(|e| format!("Pty.write: {e}"))?;
                Ok(vec![Value::Int(n as i64)])
            })));
            methods.insert("resize".to_string(), native("Pty.resize", |a| with_term(a, "Pty.resize", |t| {
                let dim = |i: usize| match a.get(i + 1) {
                    Some(Value::Int(n)) if (1..=u16::MAX as i64).contains(n) => Ok(*n as u16),
                    Some(other) => Err(super::wrong_arg("Pty.resize", i, "a positive `integer`", other)),
                    None => Err(super::missing_arg("Pty.resize", i)),
                };
                t.resize(dim(0)?, dim(1)?).map_err(|e| format!("Pty.resize: {e}"))?;
                Ok(Vec::new())
            })));
            methods.insert("poll".to_string(), native("Pty.poll", |a| with_term(a, "Pty.poll", |t| {
                Ok(vec![t.poll().map_or(Value::Nil, Value::Int)])
            })));
            methods.insert("running".to_string(), native("Pty.running", |a| with_term(a, "Pty.running", |t| {
                Ok(vec![Value::Bool(t.poll().is_none())])
            })));
            methods.insert("pid".to_string(), native("Pty.pid", |a| with_term(a, "Pty.pid", |t| {
                Ok(vec![Value::Int(i64::from(t.pid()))])
            })));
            methods.insert("kill".to_string(), native("Pty.kill", |a| with_term(a, "Pty.kill", |t| {
                Ok(vec![Value::Bool(t.kill())])
            })));
            methods.insert("atEof".to_string(), native("Pty.atEof", |a| with_term(a, "Pty.atEof", |t| {
                Ok(vec![Value::Bool(t.at_eof())])
            })));
            methods.insert("close".to_string(), native("Pty.close", |a| with_term(a, "Pty.close", |t| {
                t.hang_up();
                Ok(Vec::new())
            })));
            Rc::new(ForeignClass {
                name: "Pty".to_string(),
                package: Rc::from(PACKAGE),
                methods,
                getters: fxmap(),
                setters: fxmap(),
            })
        };
    }
    CLASS.with(Rc::clone)
}

unsafe extern "C" fn release(ptr: *mut c_void) {
    // SAFETY: `ptr` came from `Box::into_raw` in `spawn`, and a
    // `ForeignObject` releases its pointer exactly once.
    drop(unsafe { Box::from_raw(ptr.cast::<Shared>()) });
}

/// Run `f` on the terminal behind the `Pty` in `args[0]`.
fn with_term(
    args: &[Value],
    func: &str,
    f: impl FnOnce(&mut sys::Term) -> Result<Vec<Value>, String>,
) -> Result<Vec<Value>, String> {
    let Some(Value::Foreign(obj)) = args.first() else {
        return Err(format!("`{func}` is called on a `Pty`"));
    };
    if &*obj.class.package != PACKAGE {
        return Err(format!("`{func}` is called on a `Pty`"));
    }
    // SAFETY: every object of this package's class was made by `spawn`
    // around a `Box<Shared>`, alive as long as `obj` is.
    let shared = unsafe { &*obj.ptr().cast::<Shared>() };
    let mut term = shared
        .try_borrow_mut()
        .map_err(|_| format!("`{func}`: the terminal is already in use"))?;
    f(&mut term)
}

/// `Os.spawnPty(program, args?)`.
pub(crate) fn spawn(args: &[Value]) -> Result<Vec<Value>, String> {
    let program = match args.first() {
        Some(Value::Str(s)) => s.as_str().to_string(),
        Some(other) => return Err(super::wrong_arg("Os.spawnPty", 0, "a `string`", other)),
        None => return Err(super::missing_arg("Os.spawnPty", 0)),
    };
    let argv: Vec<String> = match args.get(1) {
        None | Some(Value::Nil) => Vec::new(),
        Some(Value::Table(t)) => t
            .borrow()
            .array
            .iter()
            .map(|v| match v {
                Value::Str(s) => Ok(s.as_str().to_string()),
                other => Err(super::wrong_arg("Os.spawnPty", 1, "a table of `string`s", other)),
            })
            .collect::<Result<_, _>>()?,
        Some(other) => return Err(super::wrong_arg("Os.spawnPty", 1, "a table of `string`s", other)),
    };
    // Like `Os.spawn`: a program that cannot be started is `nil`, not a
    // failure — a terminal shows that and carries on.
    let Ok(term) = sys::Term::spawn(&program, &argv, START_COLS, START_ROWS) else {
        return Ok(vec![Value::Nil]);
    };
    let ptr = Box::into_raw(Box::new(RefCell::new(term))).cast::<c_void>();
    // SAFETY: a fresh `Box<Shared>` this object now owns; `release` frees it.
    let obj = unsafe { ForeignObject::adopt(ptr, pty_class(), release) };
    Ok(vec![Value::Foreign(Rc::new(obj))])
}

/// Register native signatures for the typechecker.
pub fn register_sigs() {
    use crate::stdlib::sigs::{register, register_module, t_named, t_nullable, t_table};
    let b = || t_named("bytes");
    let i = || t_named("integer");
    let bool_ = || t_named("boolean");
    let nil = || t_named("nil");
    // A value type, like `Process`: made by `Os.spawnPty`, its methods
    // reached through a value.
    register_module("Pty");
    register(
        "Os.spawnPty",
        vec![t_named("string"), t_nullable(t_table(t_named("string")))],
        vec![t_nullable(t_named("Pty"))],
    );
    register("Pty.read", vec![t_nullable(i())], vec![b()]);
    register("Pty.write", vec![b()], vec![i()]);
    register("Pty.resize", vec![i(), i()], vec![nil()]);
    register("Pty.poll", vec![], vec![t_nullable(i())]);
    register("Pty.running", vec![], vec![bool_()]);
    register("Pty.pid", vec![], vec![i()]);
    register("Pty.kill", vec![], vec![bool_()]);
    register("Pty.atEof", vec![], vec![bool_()]);
    register("Pty.close", vec![], vec![nil()]);
}

// ---------------------------------------------------------------------------
// Unix: openpty
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod sys {
    use std::fs::File;
    use std::io::{self, Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    use std::process::{Child, Command, Stdio};

    pub(super) struct Term {
        master: File,
        child: Option<Child>,
        pid: u32,
        exit: Option<i64>,
        /// The program's side has closed and everything it wrote was read.
        eof: bool,
    }

    fn winsize(cols: u16, rows: u16) -> libc::winsize {
        libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }
    }

    fn check(r: libc::c_int) -> io::Result<libc::c_int> {
        if r < 0 { Err(io::Error::last_os_error()) } else { Ok(r) }
    }

    impl Term {
        pub(super) fn spawn(program: &str, args: &[String], cols: u16, rows: u16) -> io::Result<Term> {
            let (mut master, mut slave) = (-1, -1);
            let mut ws = winsize(cols, rows);
            // Safety: openpty writes two descriptors; the name and termios
            // are not asked for.
            check(unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut ws,
                )
            })?;
            // Safety: fresh descriptors, owned by nothing else.
            let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
            // Safety: flag changes on descriptors we own. Close-on-exec so
            // other programs this process starts do not inherit the
            // terminal; non-blocking so reads never wait.
            unsafe {
                libc::fcntl(master.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
                libc::fcntl(slave.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
                let flags = libc::fcntl(master.as_raw_fd(), libc::F_GETFL);
                libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK);
            }
            let slave = File::from(slave);
            let mut command = Command::new(program);
            command
                .args(args)
                .stdin(Stdio::from(slave.try_clone()?))
                .stdout(Stdio::from(slave.try_clone()?))
                .stderr(Stdio::from(slave));
            // SAFETY: only async-signal-safe calls between fork and exec.
            unsafe {
                command.pre_exec(|| {
                    // A session of its own, with this terminal as its
                    // controlling terminal: what makes Ctrl-C a signal and
                    // job control work.
                    check(libc::setsid())?;
                    check(libc::ioctl(0, libc::TIOCSCTTY as _, 0))?;
                    Ok(())
                });
            }
            let child = command.spawn()?;
            // The parent's copies of the program's side close with
            // `command`, so the program closing its side is our end of file.
            drop(command);
            Ok(Term {
                master: File::from(master),
                pid: child.id(),
                child: Some(child),
                exit: None,
                eof: false,
            })
        }

        pub(super) fn read(&mut self, max: usize) -> Vec<u8> {
            let mut out = Vec::new();
            let mut buf = [0u8; 16 * 1024];
            while out.len() < max && !self.eof {
                let want = buf.len().min(max - out.len());
                match self.master.read(&mut buf[..want]) {
                    Ok(0) => self.eof = true,
                    Ok(n) => out.extend_from_slice(&buf[..n]),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    // Linux reports the program's side closing as EIO.
                    Err(_) => self.eof = true,
                }
            }
            out
        }

        pub(super) fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            let mut written = 0;
            while written < data.len() {
                match self.master.write(&data[written..]) {
                    Ok(n) => written += n,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(e),
                }
            }
            Ok(written)
        }

        pub(super) fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
            let ws = winsize(cols, rows);
            // Safety: TIOCSWINSZ reads one `winsize`.
            check(unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ as _, &ws) })?;
            Ok(())
        }

        pub(super) fn pid(&self) -> u32 {
            self.pid
        }

        pub(super) fn poll(&mut self) -> Option<i64> {
            if self.exit.is_none()
                && let Some(child) = self.child.as_mut()
                && let Ok(Some(status)) = child.try_wait()
            {
                self.exit = Some(match status.code() {
                    Some(code) => i64::from(code),
                    None => 128 + i64::from(status.signal().unwrap_or(0)),
                });
            }
            self.exit
        }

        pub(super) fn kill(&mut self) -> bool {
            self.child.as_mut().is_some_and(|c| c.kill().is_ok())
        }

        pub(super) fn at_eof(&mut self) -> bool {
            self.poll().is_some() && self.eof
        }

        pub(super) fn hang_up(&mut self) {
            if self.poll().is_none() {
                // Safety: a signal to our own child.
                unsafe { libc::kill(self.pid as libc::pid_t, libc::SIGHUP) };
            }
        }
    }

    impl Drop for Term {
        fn drop(&mut self) {
            self.hang_up();
            // Reap it when it goes, without making the program wait now.
            if self.exit.is_none()
                && let Some(mut child) = self.child.take()
            {
                std::thread::spawn(move || child.wait());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Windows: ConPTY
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod sys {
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::sync::{Arc, Mutex};

    use windows_sys::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
    use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};
    use windows_sys::Win32::System::Console::{
        COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole,
    };
    use windows_sys::Win32::System::Pipes::CreatePipe;
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
        GetExitCodeProcess, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
        PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, STARTUPINFOEXW,
        TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
    };

    /// What the reader thread has collected: the bytes, and whether the
    /// output has ended.
    type Output = Arc<Mutex<(Vec<u8>, bool)>>;

    pub(super) struct Term {
        /// Closed once the program has exited, which is what ends the output.
        console: Option<HPCON>,
        input: OwnedHandle,
        process: OwnedHandle,
        pid: u32,
        output: Output,
        exit: Option<i64>,
    }

    fn owned(h: HANDLE) -> OwnedHandle {
        // SAFETY: called on fresh handles from the calls above, owned by
        // nothing else.
        unsafe { OwnedHandle::from_raw_handle(h as _) }
    }

    fn pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
        let (mut read, mut write): (HANDLE, HANDLE) = (std::ptr::null_mut(), std::ptr::null_mut());
        // SAFETY: CreatePipe writes two handles.
        if unsafe { CreatePipe(&mut read, &mut write, std::ptr::null(), 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((owned(read), owned(write)))
    }

    /// One argument, quoted the way `CommandLineToArgvW` reads it back.
    fn quote(arg: &str, out: &mut String) {
        if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
            out.push_str(arg);
            return;
        }
        out.push('"');
        let mut backslashes = 0;
        for c in arg.chars() {
            match c {
                '\\' => backslashes += 1,
                '"' => {
                    out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                    out.push('"');
                    backslashes = 0;
                }
                c => {
                    out.extend(std::iter::repeat_n('\\', backslashes));
                    out.push(c);
                    backslashes = 0;
                }
            }
        }
        out.extend(std::iter::repeat_n('\\', backslashes * 2));
        out.push('"');
    }

    fn coord(cols: u16, rows: u16) -> COORD {
        COORD {
            X: cols as i16,
            Y: rows as i16,
        }
    }

    impl Term {
        pub(super) fn spawn(program: &str, args: &[String], cols: u16, rows: u16) -> io::Result<Term> {
            let (in_read, input) = pipe()?;
            let (output_read, out_write) = pipe()?;
            let mut console: HPCON = Default::default();
            // SAFETY: the pipe ends are valid; the console handle is written.
            let hr = unsafe {
                CreatePseudoConsole(
                    coord(cols, rows),
                    in_read.as_raw_handle() as HANDLE,
                    out_write.as_raw_handle() as HANDLE,
                    0,
                    &mut console,
                )
            };
            if hr < 0 {
                return Err(io::Error::from_raw_os_error(hr));
            }
            // The console holds its own copies; ours must go, or the output
            // never ends.
            drop((in_read, out_write));

            let mut line = String::new();
            quote(program, &mut line);
            for a in args {
                line.push(' ');
                quote(a, &mut line);
            }
            let mut wide: Vec<u16> = line.encode_utf16().chain(Some(0)).collect();

            // SAFETY: the attribute list is sized by the first call, filled
            // by the second, and outlives CreateProcessW; every handle
            // written is checked before use.
            let info = unsafe {
                let mut size = 0usize;
                InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut size);
                let mut list = vec![0u8; size];
                let attrs = list.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
                if InitializeProcThreadAttributeList(attrs, 1, 0, &mut size) == 0 {
                    ClosePseudoConsole(console);
                    return Err(io::Error::last_os_error());
                }
                if UpdateProcThreadAttribute(
                    attrs,
                    0,
                    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                    console as *const std::ffi::c_void,
                    std::mem::size_of::<HPCON>(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                ) == 0
                {
                    DeleteProcThreadAttributeList(attrs);
                    ClosePseudoConsole(console);
                    return Err(io::Error::last_os_error());
                }
                let mut si: STARTUPINFOEXW = std::mem::zeroed();
                si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
                si.lpAttributeList = attrs;
                let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
                let ok = CreateProcessW(
                    std::ptr::null(),
                    wide.as_mut_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    EXTENDED_STARTUPINFO_PRESENT,
                    std::ptr::null(),
                    std::ptr::null(),
                    &si.StartupInfo,
                    &mut pi,
                );
                DeleteProcThreadAttributeList(attrs);
                if ok == 0 {
                    let e = io::Error::last_os_error();
                    ClosePseudoConsole(console);
                    return Err(e);
                }
                drop(owned(pi.hThread));
                pi
            };

            // The output pipe has no non-blocking mode: a thread reads it.
            let output: Output = Arc::new(Mutex::new((Vec::new(), false)));
            let collected = Arc::clone(&output);
            std::thread::spawn(move || {
                let mut buf = [0u8; 16 * 1024];
                loop {
                    let mut n = 0u32;
                    // SAFETY: reading into our buffer from our pipe end.
                    let ok = unsafe {
                        ReadFile(
                            output_read.as_raw_handle() as HANDLE,
                            buf.as_mut_ptr(),
                            buf.len() as u32,
                            &mut n,
                            std::ptr::null_mut(),
                        )
                    };
                    let mut out = collected.lock().unwrap_or_else(|p| p.into_inner());
                    if ok == 0 || n == 0 {
                        out.1 = true;
                        return;
                    }
                    out.0.extend_from_slice(&buf[..n as usize]);
                }
            });

            Ok(Term {
                console: Some(console),
                input,
                process: owned(info.hProcess),
                pid: info.dwProcessId,
                output,
                exit: None,
            })
        }

        pub(super) fn read(&mut self, max: usize) -> Vec<u8> {
            self.poll();
            let mut out = self.output.lock().unwrap_or_else(|p| p.into_inner());
            let n = out.0.len().min(max);
            out.0.drain(..n).collect()
        }

        pub(super) fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            let mut n = 0u32;
            // SAFETY: writing our bytes to our pipe end.
            let ok = unsafe {
                WriteFile(
                    self.input.as_raw_handle() as HANDLE,
                    data.as_ptr(),
                    data.len() as u32,
                    &mut n,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(n as usize)
        }

        pub(super) fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
            let Some(console) = self.console else {
                return Ok(());
            };
            // SAFETY: a console we own.
            let hr = unsafe { ResizePseudoConsole(console, coord(cols, rows)) };
            if hr < 0 {
                return Err(io::Error::from_raw_os_error(hr));
            }
            Ok(())
        }

        pub(super) fn pid(&self) -> u32 {
            self.pid
        }

        pub(super) fn poll(&mut self) -> Option<i64> {
            if self.exit.is_none() {
                let h = self.process.as_raw_handle() as HANDLE;
                // SAFETY: a process handle we own.
                if unsafe { WaitForSingleObject(h, 0) } == WAIT_OBJECT_0 {
                    let mut code = 0u32;
                    // SAFETY: as above; writes one exit code.
                    unsafe { GetExitCodeProcess(h, &mut code) };
                    self.exit = Some(i64::from(code as i32));
                    // The program has gone: closing the console ends the
                    // output, after what it already wrote.
                    self.close_console();
                }
            }
            self.exit
        }

        fn close_console(&mut self) {
            if let Some(console) = self.console.take() {
                // SAFETY: a console we own, closed once.
                unsafe { ClosePseudoConsole(console) };
            }
        }

        pub(super) fn kill(&mut self) -> bool {
            // SAFETY: a process handle we own.
            unsafe { TerminateProcess(self.process.as_raw_handle() as HANDLE, 1) != 0 }
        }

        pub(super) fn at_eof(&mut self) -> bool {
            self.poll().is_some() && {
                let out = self.output.lock().unwrap_or_else(|p| p.into_inner());
                out.1 && out.0.is_empty()
            }
        }

        pub(super) fn hang_up(&mut self) {
            self.close_console();
        }
    }

    impl Drop for Term {
        fn drop(&mut self) {
            self.close_console();
        }
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::sys::Term;
    use std::time::{Duration, Instant};

    fn read_until(t: &mut Term, want: &str) -> String {
        let mut all = String::new();
        let start = Instant::now();
        while !all.contains(want) && start.elapsed() < Duration::from_secs(10) {
            all.push_str(&String::from_utf8_lossy(&t.read(usize::MAX)));
            std::thread::sleep(Duration::from_millis(10));
        }
        all
    }

    #[test]
    fn a_program_runs_behind_the_console_and_its_output_arrives() {
        let mut t = Term::spawn("cmd.exe", &["/c".into(), "echo hello-pty".into()], 80, 24)
            .expect("start cmd.exe");
        assert!(t.pid() > 0);
        let out = read_until(&mut t, "hello-pty");
        assert!(out.contains("hello-pty"), "{out:?}");
        let start = Instant::now();
        while !t.at_eof() && start.elapsed() < Duration::from_secs(10) {
            t.read(usize::MAX);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(t.poll(), Some(0));
        assert!(t.at_eof());
    }

    #[test]
    fn keystrokes_reach_the_program_and_a_resize_is_accepted() {
        let mut t = Term::spawn("cmd.exe", &["/q".into()], 80, 24).expect("start cmd.exe");
        t.resize(120, 40).expect("resize");
        t.write(b"echo typed-in\r\nexit 5\r\n").expect("write");
        let out = read_until(&mut t, "typed-in");
        assert!(out.contains("typed-in"), "{out:?}");
        let start = Instant::now();
        while t.poll().is_none() && start.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(t.poll(), Some(5));
    }

    #[test]
    fn a_program_that_does_not_exist_cannot_start() {
        assert!(Term::spawn("no-such-program-anywhere.exe", &[], 80, 24).is_err());
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::sys::Term;
    use std::time::{Duration, Instant};

    /// Read until `want` appears or five seconds pass.
    fn read_until(t: &mut Term, want: &str) -> String {
        let mut all = String::new();
        let start = Instant::now();
        while !all.contains(want) && start.elapsed() < Duration::from_secs(5) {
            all.push_str(&String::from_utf8_lossy(&t.read(usize::MAX)));
            std::thread::sleep(Duration::from_millis(10));
        }
        all
    }

    fn sh(script: &str) -> Term {
        Term::spawn("/bin/sh", &["-c".into(), script.into()], 80, 24).expect("start sh")
    }

    #[test]
    fn the_program_sees_a_terminal_of_the_size_given() {
        let mut t = sh("[ -t 0 ] && [ -t 1 ] && echo tty-yes; stty size; echo done");
        let out = read_until(&mut t, "done");
        assert!(out.contains("tty-yes"), "{out:?}");
        assert!(out.contains("24 80"), "{out:?}");
    }

    #[test]
    fn a_resize_reaches_the_program() {
        let mut t = sh("read x; stty size; echo done");
        t.resize(132, 50).unwrap();
        t.write(b"go\n").unwrap();
        let out = read_until(&mut t, "done");
        assert!(out.contains("50 132"), "{out:?}");
    }

    #[test]
    fn keystrokes_go_in_and_are_echoed_as_a_terminal_does() {
        let mut t = sh("read line; echo \"got:$line\"");
        t.write(b"hello\n").unwrap();
        let out = read_until(&mut t, "got:hello");
        // The terminal echoes what was typed, then the program's answer.
        assert!(out.contains("hello\r\n"), "{out:?}");
        assert!(out.contains("got:hello"), "{out:?}");
    }

    #[test]
    fn ctrl_c_interrupts_the_program_like_a_terminal() {
        let mut t = sh("trap 'echo interrupted; exit 3' INT; echo ready; sleep 10");
        read_until(&mut t, "ready");
        t.write(&[0x03]).unwrap();
        let out = read_until(&mut t, "interrupted");
        assert!(out.contains("interrupted"), "{out:?}");
        let start = Instant::now();
        while t.poll().is_none() && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(t.poll(), Some(3));
    }

    #[test]
    fn the_end_is_seen_once_everything_was_read() {
        let mut t = sh("echo last words");
        let out = read_until(&mut t, "last words");
        assert!(out.contains("last words"));
        let start = Instant::now();
        while !t.at_eof() && start.elapsed() < Duration::from_secs(5) {
            t.read(usize::MAX);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(t.at_eof());
        assert_eq!(t.poll(), Some(0));
    }

    #[test]
    fn a_program_that_does_not_exist_cannot_start() {
        assert!(Term::spawn("/no/such/program", &[], 80, 24).is_err());
    }
}
