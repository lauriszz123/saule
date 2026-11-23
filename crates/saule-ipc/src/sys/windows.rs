//! The transport on Windows: named pipes, driven through I/O completion
//! ports by mio, plus the Win32 calls Unix gets from its socket API. See
//! `super` for what each piece must do.
//!
//! ## Handles
//!
//! A Unix socket carries file descriptors beside the bytes; a pipe carries
//! only bytes. So a handle is *duplicated into the receiving process* with
//! `DuplicateHandle` before the message goes, and the frame carries the
//! value it has there:
//!
//! ```text
//! [payload length: u32] [handle count: u32] [handle: u64]… [payload]
//! ```
//!
//! The receiver owns those handles from then on — the way it owns
//! descriptors arriving by `SCM_RIGHTS`. Duplicating into a process needs a
//! handle to it with `PROCESS_DUP_HANDLE`, opened when the connection is
//! made, from the peer's process id as the pipe reports it. That is the
//! scheme COM and Chromium use.
//!
//! A pipe takes local clients only (`PIPE_REJECT_REMOTE_CLIENTS`), and its
//! default security lets only its creator's user — and administrators —
//! write to it, so a peer is always this user: no one who could not already
//! reach into this process directly.

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::io::{self, Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, IntoRawHandle, OwnedHandle, RawHandle};
use std::ptr::NonNull;
use std::time::{Duration, Instant};

use mio::windows::NamedPipe;
use mio::{Events, Interest, Poll, Token};
use windows_sys::Win32::Foundation::{
    DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_ACCESS_DENIED, ERROR_PIPE_BUSY, HANDLE,
    INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX,
};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_READ, FILE_MAP_WRITE, MEMORY_BASIC_INFORMATION,
    MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, PAGE_READWRITE, UnmapViewOfFile, VirtualQuery,
};
use windows_sys::Win32::System::Pipes::{
    CreateNamedPipeW, GetNamedPipeClientProcessId, GetNamedPipeServerProcessId,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
    PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, OpenProcess, PROCESS_DUP_HANDLE,
};

use crate::channel::{MAX_BACKLOG, MAX_FDS, MAX_MESSAGE};
use crate::services::{self, Peer};

/// What carries an open file or shared memory between processes.
pub type Handle = OwnedHandle;

/// Length and handle count, ahead of every message.
const HEADER: usize = 8;

/// How long a client keeps retrying a service whose every pipe instance is
/// busy — the moment between the server accepting one client and opening
/// the instance for the next.
const BUSY_RETRY: Duration = Duration::from_secs(5);

// --- Waiting ---------------------------------------------------------------------

/// Waits on every pipe at once, through mio's completion port.
///
/// Every pipe is registered when it is made, for both directions; which one
/// woke the poll does not matter, because the looper tries every connection
/// after any wake-up.
pub(crate) struct Poller {
    poll: Poll,
    events: Events,
    next: usize,
}

impl Poller {
    pub(crate) fn new() -> io::Result<Poller> {
        Ok(Poller {
            poll: Poll::new()?,
            events: Events::with_capacity(256),
            next: 0,
        })
    }

    fn register(&mut self, pipe: &mut NamedPipe) -> io::Result<()> {
        let token = Token(self.next);
        self.next += 1;
        self.poll
            .registry()
            .register(pipe, token, Interest::READABLE | Interest::WRITABLE)
    }

    /// Wait until a pipe has something to do, or `timeout` passes.
    pub(crate) fn wait(
        &mut self,
        _listeners: &[&Listener],
        _channels: &[&Channel],
        timeout: Option<Duration>,
    ) -> io::Result<()> {
        loop {
            match self.poll.poll(&mut self.events, timeout) {
                Ok(()) => return Ok(()),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
    }
}

// --- Services --------------------------------------------------------------------

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(Some(0)).collect()
}

/// One instance of a service's pipe, waiting for a client.
fn pipe_instance(name: &[u16], first: bool) -> io::Result<NamedPipe> {
    let mut open_mode = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED;
    if first {
        // Fails if any instance of the name exists: two publishers of one
        // name is an error, not a second server answering half the calls.
        open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    // Safety: `name` is NUL-terminated; the result is checked.
    let h = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            65536,
            65536,
            0,
            std::ptr::null(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // Safety: a fresh pipe handle, owned by nothing else.
    Ok(unsafe { NamedPipe::from_raw_handle(h as RawHandle) })
}

/// Start an instance listening. Connected already is fine: `accept` finds it.
fn listen(pipe: &NamedPipe) -> io::Result<()> {
    match pipe.connect() {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(()),
        Err(e) => Err(e),
    }
}

/// A published service: always one pipe instance waiting for the next
/// client.
pub(crate) struct Listener {
    pending: NamedPipe,
    name: Vec<u16>,
}

impl Listener {
    /// Publish service `name`.
    pub(crate) fn bind(name: &str, poller: &mut Poller) -> io::Result<Listener> {
        let path = wide(&services::pipe_name(name)?);
        let mut pending = pipe_instance(&path, true).map_err(|e| {
            if e.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) {
                io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("the service `{name}` is already published"),
                )
            } else {
                e
            }
        })?;
        poller.register(&mut pending)?;
        listen(&pending)?;
        Ok(Listener {
            pending,
            name: path,
        })
    }

    /// The next client that has connected, without waiting.
    pub(crate) fn accept(&mut self, poller: &mut Poller) -> io::Result<Option<Channel>> {
        match self.pending.connect() {
            Ok(()) => {
                // Open the next instance before handing this one over, so a
                // client arriving meanwhile finds one.
                let mut next = pipe_instance(&self.name, false)?;
                poller.register(&mut next)?;
                listen(&next)?;
                let pipe = std::mem::replace(&mut self.pending, next);
                Channel::new(pipe).map(Some)
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(_) => {
                // A client that came and went, or a broken instance: start
                // this one over rather than stop listening.
                let _ = self.pending.disconnect();
                listen(&self.pending)?;
                Ok(None)
            }
        }
    }
}

/// Connect to service `name`.
pub(crate) fn connect(name: &str, poller: &mut Poller) -> io::Result<Channel> {
    let path = services::pipe_name(name)?;
    let deadline = Instant::now() + BUSY_RETRY;
    let file = loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(FILE_FLAG_OVERLAPPED)
            .open(&path)
        {
            Ok(file) => break file,
            Err(e)
                if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32)
                    && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("no service `{name}` is published"),
                ));
            }
            Err(e) => return Err(e),
        }
    };
    // Safety: the file is a pipe client handle opened for overlapped I/O,
    // whose ownership `into_raw_handle` hands over.
    let mut pipe = unsafe { NamedPipe::from_raw_handle(file.into_raw_handle()) };
    poller.register(&mut pipe)?;
    Channel::new(pipe)
}

// --- Connections -------------------------------------------------------------------

struct Outgoing {
    bytes: Vec<u8>,
    sent: usize,
}

/// One connection: framed messages over a byte-mode pipe.
pub(crate) struct Channel {
    pipe: NamedPipe,
    peer: Option<Peer>,
    /// The peer's process, opened for `DuplicateHandle` into it. `None` when
    /// it could not be opened; sending a handle then fails, nothing else.
    peer_process: Option<OwnedHandle>,
    inbuf: Vec<u8>,
    out: VecDeque<Outgoing>,
    out_bytes: usize,
}

impl Channel {
    fn new(pipe: NamedPipe) -> io::Result<Channel> {
        let raw = pipe.as_raw_handle() as HANDLE;
        let (mut client, mut server) = (0u32, 0u32);
        // Safety: each writes one process id; failures leave the zero.
        let me = unsafe {
            GetNamedPipeClientProcessId(raw, &mut client);
            GetNamedPipeServerProcessId(raw, &mut server);
            GetCurrentProcessId()
        };
        // Whichever end is not this process — or this process, when both
        // ends are (a program calling a service it published itself).
        let pid = if client != 0 && client != me { client } else { server };
        let peer = (pid != 0).then_some(Peer {
            pid: Some(pid),
            uid: 0,
            gid: 0,
        });
        let peer_process = (pid != 0)
            .then(|| {
                // Safety: the result is checked before it is owned.
                let h = unsafe { OpenProcess(PROCESS_DUP_HANDLE, 0, pid) };
                (!h.is_null()).then(|| unsafe { OwnedHandle::from_raw_handle(h as RawHandle) })
            })
            .flatten();
        Ok(Channel {
            pipe,
            peer,
            peer_process,
            inbuf: Vec::new(),
            out: VecDeque::new(),
            out_bytes: 0,
        })
    }

    pub(crate) fn peer(&self) -> Option<Peer> {
        self.peer
    }

    /// Queue a message and send as much as the pipe takes now.
    pub(crate) fn send(&mut self, payload: Vec<u8>, handles: Vec<OwnedHandle>) -> io::Result<()> {
        if HEADER + 8 * handles.len() + payload.len() > MAX_MESSAGE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("message of {} bytes is over the limit", payload.len()),
            ));
        }
        if handles.len() > MAX_FDS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("message carries {} handles, over the limit", handles.len()),
            ));
        }
        let mut values = Vec::with_capacity(handles.len());
        if !handles.is_empty() {
            let target = self.peer_process.as_ref().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "cannot hand a handle to this process: it could not be opened",
                )
            })?;
            for h in handles {
                let mut theirs: HANDLE = std::ptr::null_mut();
                // Safety: duplicating a handle we own into a process handle
                // we own; `theirs` is written on success.
                let ok = unsafe {
                    DuplicateHandle(
                        GetCurrentProcess(),
                        h.as_raw_handle() as HANDLE,
                        target.as_raw_handle() as HANDLE,
                        &mut theirs,
                        0,
                        0,
                        DUPLICATE_SAME_ACCESS,
                    )
                };
                if ok == 0 {
                    let e = io::Error::last_os_error();
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("cannot hand a handle to the other process: {e}"),
                    ));
                }
                values.push(theirs as usize as u64);
                // Ours closes here: the peer has its own now.
            }
        }
        let mut bytes = Vec::with_capacity(HEADER + 8 * values.len() + payload.len());
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(values.len() as u32).to_le_bytes());
        for v in &values {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        bytes.extend_from_slice(&payload);
        self.out_bytes += bytes.len();
        self.out.push_back(Outgoing { bytes, sent: 0 });
        self.flush()?;
        if self.out_bytes > MAX_BACKLOG {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "the peer has stopped reading",
            ));
        }
        Ok(())
    }

    /// Whether messages are waiting for the pipe to take them.
    pub(crate) fn wants_write(&self) -> bool {
        !self.out.is_empty()
    }

    /// Send what is queued, as far as the pipe takes it. mio holds one write
    /// in flight and reports `WouldBlock` until it completes.
    pub(crate) fn flush(&mut self) -> io::Result<()> {
        while let Some(front) = self.out.front_mut() {
            match (&self.pipe).write(&front.bytes[front.sent..]) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => {
                    front.sent += n;
                    self.out_bytes -= n;
                    if front.sent == front.bytes.len() {
                        self.out.pop_front();
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Read everything the pipe has. `Ok(false)` when the peer has closed
    /// its end.
    pub(crate) fn fill(&mut self) -> io::Result<bool> {
        let mut buf = [0u8; 64 * 1024];
        loop {
            match (&self.pipe).read(&mut buf) {
                Ok(0) => return Ok(false),
                Ok(n) => self.inbuf.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(true),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) if e.kind() == io::ErrorKind::BrokenPipe => return Ok(false),
                Err(e) => return Err(e),
            }
        }
    }

    /// The next whole message read so far, with its handles.
    pub(crate) fn next_message(&mut self) -> io::Result<Option<(Vec<u8>, Vec<OwnedHandle>)>> {
        if self.inbuf.len() < HEADER {
            return Ok(None);
        }
        let len = u32::from_le_bytes(self.inbuf[0..4].try_into().unwrap()) as usize;
        let count = u32::from_le_bytes(self.inbuf[4..8].try_into().unwrap()) as usize;
        if count > MAX_FDS || HEADER + 8 * count + len > MAX_MESSAGE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "a message over the limits",
            ));
        }
        let total = HEADER + 8 * count + len;
        if self.inbuf.len() < total {
            return Ok(None);
        }
        let mut handles = Vec::with_capacity(count);
        for i in 0..count {
            let at = HEADER + 8 * i;
            let v = u64::from_le_bytes(self.inbuf[at..at + 8].try_into().unwrap());
            if v == 0 || v as usize as HANDLE == INVALID_HANDLE_VALUE {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "a message naming no handle",
                ));
            }
            // Safety: the peer duplicated this handle into this process for
            // this message; it is ours from here (see the module docs).
            handles.push(unsafe { OwnedHandle::from_raw_handle(v as usize as RawHandle) });
        }
        let payload = self.inbuf[HEADER + 8 * count..total].to_vec();
        self.inbuf.drain(..total);
        Ok(Some((payload, handles)))
    }
}

// --- Shared memory -------------------------------------------------------------------

/// A new unnamed section of `len` bytes, backed by the paging file.
pub(crate) fn shm_create(len: usize) -> io::Result<OwnedHandle> {
    let len = len as u64;
    // Safety: an anonymous section; the result is checked.
    let h = unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            std::ptr::null(),
            PAGE_READWRITE,
            (len >> 32) as u32,
            len as u32,
            std::ptr::null(),
        )
    };
    if h.is_null() {
        return Err(io::Error::last_os_error());
    }
    // Safety: a fresh section handle, owned by nothing else.
    Ok(unsafe { OwnedHandle::from_raw_handle(h as RawHandle) })
}

/// Map `len` bytes of the section behind `handle`, refusing when it is
/// smaller — so a peer cannot make us read past the end of what it really
/// shares.
pub(crate) fn shm_map(handle: &OwnedHandle, len: usize) -> io::Result<NonNull<u8>> {
    // Safety: mapping a section handle we own; the result is checked.
    let view = unsafe {
        MapViewOfFile(
            handle.as_raw_handle() as HANDLE,
            FILE_MAP_READ | FILE_MAP_WRITE,
            0,
            0,
            len,
        )
    };
    let Some(ptr) = NonNull::new(view.Value.cast::<u8>()) else {
        return Err(io::Error::last_os_error());
    };
    // Safety: asking about memory we just mapped.
    let size = unsafe {
        let mut info: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
        VirtualQuery(
            view.Value,
            &mut info,
            std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
        );
        info.RegionSize
    };
    if size < len {
        shm_unmap(ptr, len);
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("shared memory of {size} bytes cannot hold {len}"),
        ));
    }
    Ok(ptr)
}

/// Undo [`shm_map`].
pub(crate) fn shm_unmap(ptr: NonNull<u8>, _len: usize) {
    // Safety: unmapping exactly the view `shm_map` mapped, once.
    unsafe {
        UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
            Value: ptr.as_ptr().cast(),
        });
    }
}
