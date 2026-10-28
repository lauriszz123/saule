//! Child-process handles backing `Value::Process`.
//!
//! `Os.execute` and `Os.capture` both block until the child is finished,
//! which makes them useless for a child you mean to *talk to*: a language
//! server, a formatter you feed on stdin, an editor embedded in a window.
//! This is the handle for that case — the child runs while the program does,
//! and reads never block.
//!
//! ## Why a reader thread
//!
//! A pipe read blocks until there is something to read, and nothing in
//! `std` makes one non-blocking portably: it is `O_NONBLOCK` on Unix and
//! overlapped IO or `PeekNamedPipe` on Windows, neither reachable without a
//! platform crate. So each captured stream gets a thread that does the
//! blocking read and appends to a shared buffer, and the Saule-visible
//! `read` takes whatever has arrived so far and returns immediately.
//!
//! That is the property a frame loop needs: a UI can call `read` once per
//! frame and never stall, which a `O_NONBLOCK` fd would also give but only
//! on one platform. The cost is a thread per stream, which for the handful
//! of children a program talks to is not a cost worth optimising.
//!
//! The threads never touch a Saule value — they own their pipe and a
//! `Mutex<StreamState>`, nothing else — so the single-threaded runtime
//! stays single-threaded.

use std::collections::VecDeque;
use std::fmt;
use std::io::Read;
use std::process::{Child, ChildStdin};
use std::sync::{Arc, Condvar, Mutex};

/// How much unread output one stream will hold before the reader thread
/// stops taking more.
///
/// A buffer has to have a limit. Without one, a child that talks faster than
/// the program reads — or one whose output is never read at all — grows this
/// until the process dies, and "my editor used 4 GB" is a worse failure than
/// any the limit causes.
///
/// 8 MiB because the limit should only ever be reached by a mistake. An
/// interactive protocol deals in kilobytes per exchange, so a program that
/// reads once a frame sits near zero; reaching 8 MiB means nobody is reading,
/// and that is the case worth bounding.
const MAX_BUFFERED: usize = 8 * 1024 * 1024;

/// What a reader thread has collected from one stream so far.
#[derive(Default)]
pub struct StreamState {
    /// Bytes read but not yet handed to the program.
    pub data: VecDeque<u8>,
    /// Set once the stream reached end of file, so a program can tell "no
    /// output yet" from "no more output ever".
    pub eof: bool,
    /// Set by [`Stream::unbound`] to lift [`MAX_BUFFERED`] — see there.
    unbounded: bool,
}

/// One captured output stream: a buffer a reader thread fills.
///
/// The condvar is what makes the limit backpressure rather than data loss.
/// When the buffer is full the reader stops reading, the operating system's
/// own pipe buffer fills behind it, and the child blocks on its next write —
/// which is what a pipe is for. Dropping bytes instead would be silent
/// corruption, and for a binary protocol an unrecoverable kind.
pub struct Stream(Arc<(Mutex<StreamState>, Condvar)>);

impl Stream {
    /// Start reading `source` on a thread of its own, into a fresh buffer.
    fn spawn_reader<R: Read + Send + 'static>(source: R) -> Stream {
        let state = Arc::new((Mutex::new(StreamState::default()), Condvar::new()));
        let sink = Arc::clone(&state);
        // Detached on purpose: the thread ends when the pipe closes, which
        // happens when the child exits or the handle drops the pipe. There
        // is nothing to join it for, and joining would block.
        std::thread::spawn(move || {
            let mut source = source;
            let mut chunk = [0u8; 8192];
            loop {
                // Wait for room before reading, so a full buffer leaves the
                // bytes in the pipe rather than dropping them here.
                {
                    let mut state = lock(&sink);
                    while state.data.len() >= MAX_BUFFERED && !state.unbounded {
                        state = sink.1.wait(state).unwrap_or_else(|e| e.into_inner());
                    }
                }
                match source.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => lock(&sink).data.extend(&chunk[..n]),
                    // A read error ends the stream. The child's exit status
                    // is the honest report of what went wrong, and it is
                    // already available through `poll`.
                    Err(_) => break,
                }
            }
            lock(&sink).eof = true;
        });
        Stream(state)
    }

    /// A stream that was never captured: permanently empty and at eof.
    fn closed() -> Stream {
        Stream(Arc::new((
            Mutex::new(StreamState {
                data: VecDeque::new(),
                eof: true,
                unbounded: false,
            }),
            Condvar::new(),
        )))
    }

    /// Take up to `max` bytes of what has arrived, leaving the rest. Returns
    /// empty when nothing has arrived yet — which is not eof; see
    /// [`Stream::at_eof`].
    pub fn take(&self, max: usize) -> Vec<u8> {
        let mut state = lock(&self.0);
        let n = max.min(state.data.len());
        let taken = state.data.drain(..n).collect();
        // Room again: wake the reader if the limit had stopped it.
        drop(state);
        self.0.1.notify_all();
        taken
    }

    /// How many bytes are waiting.
    pub fn available(&self) -> usize {
        lock(&self.0).data.len()
    }

    /// Lift the buffer limit for the rest of this stream's life.
    ///
    /// Used by [`ProcessHandle::wait`], which means "let it run to
    /// completion" — and a child held back by a full buffer would never
    /// reach completion, so waiting on one while the limit applied would
    /// hang. Buffering without a bound is the right trade *there* because
    /// the caller has asked to block until the child is done, which is also
    /// what `Os.capture` does. It is the wrong trade for the polling case,
    /// where nobody has agreed to wait for anything.
    fn unbound(&self) {
        lock(&self.0).unbounded = true;
        self.0.1.notify_all();
    }

    /// Has the stream ended *and* been drained? Both halves matter: a child
    /// can exit with output still sitting in the buffer, and reporting eof
    /// then would lose it.
    pub fn at_eof(&self) -> bool {
        let state = lock(&self.0);
        state.eof && state.data.is_empty()
    }
}

/// Lock through a poisoned mutex rather than panicking.
///
/// The only code that can poison one of these is the reader thread above,
/// and it holds the lock across nothing but a `VecDeque::extend`. If that
/// panicked the allocator is gone and the buffer's contents are the least of
/// it — so recovering the data is strictly better than turning the panic
/// into a second one here.
fn lock(m: &Arc<(Mutex<StreamState>, Condvar)>) -> std::sync::MutexGuard<'_, StreamState> {
    m.0.lock().unwrap_or_else(|e| e.into_inner())
}

/// A running (or finished) child process.
pub struct ProcessHandle {
    /// The child, until it has been reaped by `wait`/`poll`.
    child: Option<Child>,
    /// Its stdin, until `closeStdin` drops it to signal end of input.
    stdin: Option<ChildStdin>,
    stdout: Stream,
    stderr: Stream,
    pid: u32,
    /// The exit code, once known. Cached because a `Child` can only be
    /// waited on once, and a program may well ask twice.
    status: Option<i64>,
    /// How it was spawned, for `Debug` and error messages.
    program: String,
}

impl ProcessHandle {
    /// Spawn `program` with `args`, capturing all three standard streams.
    ///
    /// The arguments are passed as a vector, never through a shell: unlike
    /// `Os.execute`, nothing here re-parses quoting, so a path with a space
    /// in it is one argument because it is one element.
    /// `cwd` sets the child's working directory, and `env` adds to or
    /// overrides entries in the environment it inherits. Overlaying rather
    /// than replacing is what a caller almost always means — a child handed
    /// an environment with no `PATH` usually cannot run at all — so there is
    /// deliberately no way here to start from an empty one.
    pub fn spawn(
        program: &str,
        args: &[String],
        cwd: Option<&str>,
        env: &[(String, String)],
    ) -> std::io::Result<ProcessHandle> {
        use std::process::{Command, Stdio};
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = cwd {
            command.current_dir(dir);
        }
        for (name, value) in env {
            command.env(name, value);
        }
        let mut child = command.spawn()?;
        let pid = child.id();
        // `take` rather than `unwrap`: a piped stdio handle is always
        // present here, but a missing one should disable that stream rather
        // than abort the program.
        let stdout = match child.stdout.take() {
            Some(s) => Stream::spawn_reader(s),
            None => Stream::closed(),
        };
        let stderr = match child.stderr.take() {
            Some(s) => Stream::spawn_reader(s),
            None => Stream::closed(),
        };
        Ok(ProcessHandle {
            stdin: child.stdin.take(),
            child: Some(child),
            stdout,
            stderr,
            pid,
            status: None,
            program: program.to_string(),
        })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn stdout(&self) -> &Stream {
        &self.stdout
    }

    pub fn stderr(&self) -> &Stream {
        &self.stderr
    }

    /// Write to the child's stdin, returning how many bytes went in.
    pub fn write(&mut self, data: &[u8]) -> Result<usize, String> {
        use std::io::Write;
        let Some(stdin) = self.stdin.as_mut() else {
            return Err("the child's stdin is closed".to_string());
        };
        // `write_all`, not `write`: a partial pipe write is not an outcome a
        // caller can do anything useful with, and the count it would report
        // invites a loop that `write_all` already is.
        match stdin.write_all(data).and_then(|()| stdin.flush()) {
            Ok(()) => Ok(data.len()),
            Err(e) => Err(format!("writing to the child's stdin failed: {e}")),
        }
    }

    /// Drop stdin, so the child sees end of input. Idempotent: a program
    /// that closes twice has still expressed one intention.
    pub fn close_stdin(&mut self) {
        self.stdin = None;
    }

    /// The exit code if the child has finished, `None` while it runs.
    pub fn poll(&mut self) -> Result<Option<i64>, String> {
        if let Some(code) = self.status {
            return Ok(Some(code));
        }
        let Some(child) = self.child.as_mut() else {
            return Ok(self.status);
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                let code = exit_code(&status);
                self.status = Some(code);
                self.child = None;
                Ok(Some(code))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("checking on `{}` failed: {e}", self.program)),
        }
    }

    /// Block until the child exits, and return its code.
    pub fn wait(&mut self) -> Result<i64, String> {
        if let Some(code) = self.status {
            return Ok(code);
        }
        // Dropping stdin first: a child that is waiting for end of input
        // and a parent that is waiting for the child is a deadlock, and
        // this is the end that can tell.
        self.stdin = None;
        // Same reasoning for the output side. A child held back by a full
        // buffer cannot exit, so waiting for it while the limit applied
        // would hang; `wait` means "run to completion", so the limit goes.
        self.stdout.unbound();
        self.stderr.unbound();
        let Some(mut child) = self.child.take() else {
            // Already reaped with no status recorded — only reachable if
            // `poll` saw an error. Report it as the failure code the rest
            // of `Os` uses.
            return Ok(self.status.unwrap_or(-1));
        };
        match child.wait() {
            Ok(status) => {
                let code = exit_code(&status);
                self.status = Some(code);
                Ok(code)
            }
            Err(e) => Err(format!("waiting for `{}` failed: {e}", self.program)),
        }
    }

    /// Ask the OS to end the child. `true` if the signal was delivered;
    /// `false` if it had already exited, which is not an error.
    pub fn kill(&mut self) -> bool {
        match self.child.as_mut() {
            Some(child) => child.kill().is_ok(),
            None => false,
        }
    }

    /// Is the child still running? Asks the OS rather than trusting a flag,
    /// so it is accurate without a prior `poll`.
    pub fn running(&mut self) -> bool {
        matches!(self.poll(), Ok(None))
    }
}

/// A process's exit code, with a signal death reported as `-1` — the same
/// value `Os.execute` gives for "it did not exit normally".
fn exit_code(status: &std::process::ExitStatus) -> i64 {
    status.code().unwrap_or(-1) as i64
}

impl fmt::Debug for ProcessHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.status {
            Some(code) => write!(f, "<process {} {} exited {code}>", self.pid, self.program),
            None => write!(f, "<process {} {}>", self.pid, self.program),
        }
    }
}
