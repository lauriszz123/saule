//! The transport on Unix (Linux, macOS): Unix-domain stream sockets that
//! carry file descriptors (`SCM_RIGHTS`), `poll`, and memfd / POSIX shared
//! memory. See `super` for what each piece must do.

use std::collections::VecDeque;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::time::{Duration, Instant};

use crate::channel::{MAX_BACKLOG, MAX_FDS, MAX_MESSAGE};
use crate::services::{self, Peer};

/// What carries an open file, socket or shared memory between processes.
pub type Handle = OwnedFd;

// --- Waiting ---------------------------------------------------------------------

/// Waits on every listener and connection at once, with `poll`.
#[derive(Default)]
pub(crate) struct Poller;

impl Poller {
    pub(crate) fn new() -> io::Result<Poller> {
        Ok(Poller)
    }

    /// Wait until a listener or connection has something to do, or
    /// `timeout` passes.
    pub(crate) fn wait(
        &mut self,
        listeners: &[&Listener],
        channels: &[&Channel],
        timeout: Option<Duration>,
    ) -> io::Result<()> {
        let mut fds: Vec<libc::pollfd> = Vec::with_capacity(listeners.len() + channels.len());
        for l in listeners {
            fds.push(libc::pollfd {
                fd: l.sock.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            });
        }
        for c in channels {
            let mut events = libc::POLLIN;
            if c.wants_write() {
                events |= libc::POLLOUT;
            }
            fds.push(libc::pollfd {
                fd: c.fd(),
                events,
                revents: 0,
            });
        }
        let deadline = timeout.map(|t| Instant::now() + t);
        loop {
            let ms = match deadline {
                None => -1,
                Some(d) => d
                    .saturating_duration_since(Instant::now())
                    .as_millis()
                    .min(i32::MAX as u128) as i32,
            };
            // Safety: `fds` is a live array of `fds.len()` pollfds.
            let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, ms) };
            if n >= 0 {
                return Ok(());
            }
            let err = io::Error::last_os_error();
            if err.kind() != io::ErrorKind::Interrupted {
                return Err(err);
            }
        }
    }
}

// --- Services --------------------------------------------------------------------

/// A published service: a listening socket in the services directory.
pub(crate) struct Listener {
    sock: UnixListener,
    path: PathBuf,
}

impl Listener {
    /// Publish service `name`.
    pub(crate) fn bind(name: &str, _poller: &mut Poller) -> io::Result<Listener> {
        let path = services::path_for(name)?;
        Listener::bind_at(&path, name)
    }

    /// Publish at an explicit socket path.
    pub(crate) fn bind_at(path: &Path, name: &str) -> io::Result<Listener> {
        let sock = bind_at(path, name)?;
        sock.set_nonblocking(true)?;
        Ok(Listener {
            sock,
            path: path.to_owned(),
        })
    }

    /// The next client waiting to connect, without waiting.
    pub(crate) fn accept(&mut self, _poller: &mut Poller) -> io::Result<Option<Channel>> {
        loop {
            match self.sock.accept() {
                Ok((sock, _)) => return Channel::new(sock).map(Some),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(None),
                Err(e) => return Err(e),
            }
        }
    }

    pub(crate) fn fd(&self) -> RawFd {
        self.sock.as_raw_fd()
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        // The socket file would otherwise outlive the service.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Bind the socket for a service.
///
/// A socket file left behind by a process that died is replaced; one that a
/// live process still answers on is not — two publishers of one name is an
/// error, not a takeover.
fn bind_at(path: &Path, name: &str) -> io::Result<UnixListener> {
    if let Some(dir) = path.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    match UnixListener::bind(path) {
        Ok(listener) => {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            Ok(listener)
        }
        Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
            if UnixStream::connect(path).is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("the service `{name}` is already published"),
                ));
            }
            std::fs::remove_file(path)?;
            let listener = UnixListener::bind(path)?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            Ok(listener)
        }
        Err(e) => Err(e),
    }
}

/// Connect to service `name`.
pub(crate) fn connect(name: &str, _poller: &mut Poller) -> io::Result<Channel> {
    let path = services::path_for(name)?;
    let sock = UnixStream::connect(&path).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => {
            io::Error::new(e.kind(), format!("no service `{name}` is published"))
        }
        _ => e,
    })?;
    Channel::new(sock)
}

/// Connect to an explicit socket path.
pub(crate) fn connect_at(path: &Path) -> io::Result<Channel> {
    Channel::new(UnixStream::connect(path)?)
}

// --- Connections -------------------------------------------------------------------

/// Length and handle count, ahead of every message.
const HEADER: usize = 8;

struct Outgoing {
    bytes: Vec<u8>,
    sent: usize,
    /// Sent with the first chunk of `bytes`, then empty.
    fds: Vec<OwnedFd>,
}

/// One connection. Descriptors ride on the first `sendmsg` of their message,
/// and the kernel delivers them with its first byte, so by the time a whole
/// message has been read its descriptors have too.
pub(crate) struct Channel {
    sock: UnixStream,
    peer: Option<Peer>,
    inbuf: Vec<u8>,
    infds: VecDeque<OwnedFd>,
    out: VecDeque<Outgoing>,
    out_bytes: usize,
}

impl Channel {
    pub(crate) fn new(sock: UnixStream) -> io::Result<Channel> {
        sock.set_nonblocking(true)?;
        // macOS has no MSG_NOSIGNAL: a write to a peer that has gone would
        // raise SIGPIPE and kill the process instead of returning EPIPE.
        #[cfg(target_vendor = "apple")]
        unsafe {
            let on: libc::c_int = 1;
            libc::setsockopt(
                sock.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_NOSIGPIPE,
                (&on as *const libc::c_int).cast(),
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            );
        }
        let peer = peer_of(&sock).ok();
        Ok(Channel {
            sock,
            peer,
            inbuf: Vec::new(),
            infds: VecDeque::new(),
            out: VecDeque::new(),
            out_bytes: 0,
        })
    }

    pub(crate) fn fd(&self) -> RawFd {
        self.sock.as_raw_fd()
    }

    pub(crate) fn peer(&self) -> Option<Peer> {
        self.peer
    }

    #[cfg(test)]
    pub(crate) fn socket(&self) -> &UnixStream {
        &self.sock
    }

    /// Queue a message and send as much as the socket takes now.
    pub(crate) fn send(&mut self, payload: Vec<u8>, fds: Vec<OwnedFd>) -> io::Result<()> {
        if HEADER + payload.len() > MAX_MESSAGE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("message of {} bytes is over the limit", payload.len()),
            ));
        }
        if fds.len() > MAX_FDS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("message carries {} descriptors, over the limit", fds.len()),
            ));
        }
        let mut bytes = Vec::with_capacity(HEADER + payload.len());
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(fds.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&payload);
        self.out_bytes += bytes.len();
        self.out.push_back(Outgoing {
            bytes,
            sent: 0,
            fds,
        });
        self.flush()?;
        if self.out_bytes > MAX_BACKLOG {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "the peer has stopped reading",
            ));
        }
        Ok(())
    }
    /// Whether messages are waiting for the socket to take them.
    pub(crate) fn wants_write(&self) -> bool {
        !self.out.is_empty()
    }

    /// Send what is queued, as far as the socket takes it.
    pub(crate) fn flush(&mut self) -> io::Result<()> {
        while let Some(front) = self.out.front_mut() {
            let rest = &front.bytes[front.sent..];
            let fds: Vec<RawFd> = front.fds.iter().map(AsRawFd::as_raw_fd).collect();
            match send_with_fds(self.sock.as_raw_fd(), rest, &fds) {
                Ok(n) => {
                    // The descriptors went with this chunk.
                    front.fds.clear();
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

    /// Read everything the socket has. `Ok(false)` when the peer has closed
    /// its end.
    pub(crate) fn fill(&mut self) -> io::Result<bool> {
        let mut buf = [0u8; 64 * 1024];
        loop {
            match recv_with_fds(self.sock.as_raw_fd(), &mut buf, &mut self.infds) {
                Ok(0) => return Ok(false),
                Ok(n) => {
                    self.inbuf.extend_from_slice(&buf[..n]);
                    if self.infds.len() > MAX_FDS * 4 {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "too many descriptors waiting",
                        ));
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(true),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
    }

    /// The next whole message read so far, with its descriptors.
    pub(crate) fn next_message(&mut self) -> io::Result<Option<(Vec<u8>, Vec<OwnedFd>)>> {
        if self.inbuf.len() < HEADER {
            return Ok(None);
        }
        let len = u32::from_le_bytes(self.inbuf[0..4].try_into().unwrap()) as usize;
        let nfds = u32::from_le_bytes(self.inbuf[4..8].try_into().unwrap()) as usize;
        if HEADER + len > MAX_MESSAGE || nfds > MAX_FDS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "a message over the limits",
            ));
        }
        if self.inbuf.len() < HEADER + len {
            return Ok(None);
        }
        if self.infds.len() < nfds {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "a message without the descriptors it announced",
            ));
        }
        let payload = self.inbuf[HEADER..HEADER + len].to_vec();
        self.inbuf.drain(..HEADER + len);
        let fds = self.infds.drain(..nfds).collect();
        Ok(Some((payload, fds)))
    }
}

/// `sendmsg` with `fds` as `SCM_RIGHTS`, never blocking, never raising
/// SIGPIPE.
fn send_with_fds(sock: RawFd, bytes: &[u8], fds: &[RawFd]) -> io::Result<usize> {
    let mut iov = libc::iovec {
        iov_base: bytes.as_ptr() as *mut libc::c_void,
        iov_len: bytes.len(),
    };
    // u64s for the alignment `cmsghdr` needs.
    let mut control = vec![0u64; cmsg_space(fds.len()).div_ceil(8)];
    // Safety: the header points at `iov` and `control`, both alive for the
    // call; the control buffer is sized by CMSG_SPACE for `fds`.
    unsafe {
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        if !fds.is_empty() {
            msg.msg_control = control.as_mut_ptr().cast();
            msg.msg_controllen = cmsg_space(fds.len()) as _;
            let cmsg = libc::CMSG_FIRSTHDR(&msg);
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of_val(fds) as u32) as _;
            std::ptr::copy_nonoverlapping(
                fds.as_ptr(),
                libc::CMSG_DATA(cmsg).cast::<RawFd>(),
                fds.len(),
            );
        }
        #[cfg(target_os = "linux")]
        let flags = libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL;
        #[cfg(not(target_os = "linux"))]
        let flags = libc::MSG_DONTWAIT;
        let n = libc::sendmsg(sock, &msg, flags);
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(n as usize)
    }
}

/// `recvmsg`, collecting any `SCM_RIGHTS` descriptors into `fds`, each
/// close-on-exec.
fn recv_with_fds(sock: RawFd, buf: &mut [u8], fds: &mut VecDeque<OwnedFd>) -> io::Result<usize> {
    let mut iov = libc::iovec {
        iov_base: buf.as_mut_ptr().cast(),
        iov_len: buf.len(),
    };
    let space = cmsg_space(MAX_FDS);
    let mut control = vec![0u64; space.div_ceil(8)];
    // Safety: as in `send_with_fds`; every descriptor the kernel wrote into
    // the control buffer is new and ours, so wrapping it in `OwnedFd` is
    // what closes it if nothing else takes it.
    unsafe {
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = space as _;
        #[cfg(target_os = "linux")]
        let flags = libc::MSG_DONTWAIT | libc::MSG_CMSG_CLOEXEC;
        #[cfg(not(target_os = "linux"))]
        let flags = libc::MSG_DONTWAIT;
        let n = libc::recvmsg(sock, &mut msg, flags);
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(cmsg).cast::<RawFd>();
                let bytes = (*cmsg).cmsg_len as usize - (data as usize - cmsg as usize);
                for i in 0..bytes / std::mem::size_of::<RawFd>() {
                    let fd = std::ptr::read_unaligned(data.add(i));
                    #[cfg(not(target_os = "linux"))]
                    libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
                    fds.push_back(OwnedFd::from_raw_fd(fd));
                }
            }
            cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
        }
        if msg.msg_flags & libc::MSG_CTRUNC != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "more descriptors than a message may carry",
            ));
        }
        Ok(n as usize)
    }
}

fn cmsg_space(fds: usize) -> usize {
    // Safety: a pure size computation.
    unsafe { libc::CMSG_SPACE((fds * std::mem::size_of::<RawFd>()) as u32) as usize }
}

#[cfg(target_os = "linux")]
pub(crate) fn peer_of(sock: &UnixStream) -> io::Result<Peer> {
    use std::os::fd::AsRawFd;
    // Safety: SO_PEERCRED writes one `ucred` through the pointer.
    unsafe {
        let mut cred: libc::ucred = std::mem::zeroed();
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        if libc::getsockopt(
            sock.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        ) != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Peer {
            pid: Some(cred.pid as u32),
            uid: cred.uid,
            gid: cred.gid,
        })
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn peer_of(sock: &UnixStream) -> io::Result<Peer> {
    use std::os::fd::AsRawFd;
    // Safety: getpeereid writes two ids; LOCAL_PEERPID one pid.
    unsafe {
        let (mut uid, mut gid) = (0, 0);
        if libc::getpeereid(sock.as_raw_fd(), &mut uid, &mut gid) != 0 {
            return Err(io::Error::last_os_error());
        }
        #[cfg(target_vendor = "apple")]
        let pid = {
            // <sys/un.h>: SOL_LOCAL 0, LOCAL_PEERPID 2.
            let mut pid: libc::pid_t = 0;
            let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
            (libc::getsockopt(
                sock.as_raw_fd(),
                0,
                2,
                (&mut pid as *mut libc::pid_t).cast(),
                &mut len,
            ) == 0)
                .then_some(pid as u32)
        };
        #[cfg(not(target_vendor = "apple"))]
        let pid = None;
        Ok(Peer { pid, uid, gid })
    }
}

// --- Shared memory -------------------------------------------------------------------

/// A new block of anonymous shared memory of `len` bytes.
pub(crate) fn shm_create(len: usize) -> io::Result<OwnedFd> {
    let fd = anonymous_fd()?;
    // Safety: `fd` is a fresh shared-memory descriptor of ours.
    if unsafe { libc::ftruncate(fd.as_raw_fd(), len as libc::off_t) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}

/// Map `len` bytes of the block behind `fd`, refusing when it is smaller —
/// so a peer cannot make us read past the end of what it really shares.
pub(crate) fn shm_map(fd: &OwnedFd, len: usize) -> io::Result<NonNull<u8>> {
    // Safety: fstat writes one `stat` through the pointer.
    let size = unsafe {
        let mut st: libc::stat = std::mem::zeroed();
        if libc::fstat(fd.as_raw_fd(), &mut st) != 0 {
            return Err(io::Error::last_os_error());
        }
        st.st_size as u64
    };
    if (len as u64) > size {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("shared memory of {size} bytes cannot hold {len}"),
        ));
    }
    // Safety: mapping `len` bytes of a descriptor we own, shared, for
    // reading and writing; checked for failure below.
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd.as_raw_fd(),
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        return Err(io::Error::last_os_error());
    }
    Ok(NonNull::new(ptr.cast()).expect("mmap does not return null on success"))
}

/// Undo [`shm_map`].
pub(crate) fn shm_unmap(ptr: NonNull<u8>, len: usize) {
    // Safety: unmapping exactly what `shm_map` mapped, once.
    unsafe { libc::munmap(ptr.as_ptr().cast(), len) };
}

#[cfg(target_os = "linux")]
fn anonymous_fd() -> io::Result<OwnedFd> {
    // Safety: the name is NUL-terminated; the result is checked.
    let fd = unsafe { libc::memfd_create(c"saule-shm".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // Safety: a fresh descriptor, owned by nothing else.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

#[cfg(not(target_os = "linux"))]
fn anonymous_fd() -> io::Result<OwnedFd> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);

    for _ in 0..16 {
        // Unique enough: pid, a counter, and the clock. The name only exists
        // between `shm_open` and `shm_unlink` below.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let name = format!(
            "/saule.{}.{}.{nanos}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        );
        let cname = std::ffi::CString::new(name).expect("no NUL in the name");
        // Safety: a NUL-terminated name; the result is checked.
        let fd = unsafe {
            libc::shm_open(
                cname.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
                0o600 as libc::c_uint,
            )
        };
        if fd < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::AlreadyExists {
                continue;
            }
            return Err(err);
        }
        // Safety: as above; the name goes, the memory stays with `fd`.
        unsafe {
            libc::shm_unlink(cname.as_ptr());
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            return Ok(OwnedFd::from_raw_fd(fd));
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no free name for shared memory",
    ))
}
