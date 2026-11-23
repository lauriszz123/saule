//! The call machinery: one per thread, owning every connection that thread
//! has — the services it publishes, the clients that connected to them, and
//! the services it connected to.
//!
//! ## Calls
//!
//! A call names an object and a method and carries arguments. Two kinds:
//!
//! - **with a reply** ([`Looper::call`], or [`Looper::call_async`] then
//!   [`Looper::wait`]): the caller waits for the result. *While it waits it
//!   keeps serving*: calls arriving meanwhile — the server calling back into
//!   the app in the middle of `createWindow`, say — run straight away, on the
//!   same thread. Waiting cannot deadlock on a callback, and still only one
//!   piece of code runs at a time, so nothing needs a lock.
//! - **one-way** ([`Looper::send`]): sent, never answered. Events go this way,
//!   so a display server never waits on an app that has stopped responding.
//!
//! Messages from one connection are handled in the order they were sent.
//!
//! ## Objects
//!
//! Objects are passed by reference (see [`super::wire`]). The looper numbers
//! the ones this process exports, per connection, and maps numbers coming
//! back to the key the layer above gave the object. What an object *is* —
//! a Saule instance — is that layer's business, reached through
//! [`Handler`].
//!
//! Releasing a reference has a race: a process can send an object again
//! while the receiver's release of it is in flight, and freeing it then would
//! strand the new reference. So the exporter counts how many times it sent
//! each object, the receiver counts how many it received, and a release
//! carries the receiver's count; the export goes only when every reference
//! sent has come back released. (Binder and Wayland both settle this the
//! same way, with slightly different counters.)
//!
//! Object `0` on every connection is the service's root — the object the
//! service published, and what `Services.connect` hands the client. It lives
//! as long as the connection.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io;
use std::time::Duration;

use super::channel::Channel;
use super::services::Peer;
use super::sys::{self, Handle};
use super::wire::{DecodeError, Decoder, Encoder, Wire};

pub type ConnId = u32;
pub type ObjectId = u32;
pub type CallId = u32;

/// The root object of a connection: what was published.
pub const ROOT: ObjectId = 0;

/// Why a call did not produce a result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    /// The connection closed before the reply came — the other process
    /// ended, crashed, or cut us off.
    Disconnected,
    /// The other side ran the call and it failed; its error message.
    Remote(String),
    /// The call could not be sent: an argument that cannot travel, or a
    /// message over the size limits.
    Invalid(String),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::Disconnected => write!(f, "the other process has disconnected"),
            CallError::Remote(m) => write!(f, "{m}"),
            CallError::Invalid(m) => write!(f, "{m}"),
        }
    }
}

/// The layer above: what objects are, and what happens to them.
pub trait Handler {
    /// Run `method` on the object exported under `key`, called from `from`.
    /// `Err` is a fault reported back to the caller as
    /// [`CallError::Remote`].
    fn call(
        &mut self,
        looper: &mut Looper,
        from: ConnId,
        key: u64,
        method: &str,
        args: Vec<Wire>,
    ) -> Result<Vec<Wire>, String>;

    /// A client connected to the service published as `service`.
    fn connected(&mut self, _looper: &mut Looper, _conn: ConnId, _service: &str) {}

    /// `conn` has closed. Every reference held over it is dead.
    fn disconnected(&mut self, _looper: &mut Looper, _conn: ConnId) {}

    /// The peer at `conn` no longer holds the object exported under `key`.
    /// Called once per connection the object had been sent over.
    fn released(&mut self, _looper: &mut Looper, _conn: ConnId, _key: u64) {}
}

enum Message {
    Call {
        call: CallId,
        object: ObjectId,
        method: String,
        args: Vec<Wire>,
    },
    Reply {
        call: CallId,
        result: Result<Vec<Wire>, String>,
    },
    Release {
        object: ObjectId,
        count: u64,
    },
}

const CALL: u8 = 1;
const REPLY_OK: u8 = 2;
const REPLY_FAULT: u8 = 3;
const RELEASE: u8 = 4;

/// Call id 0 is a one-way call: no reply wanted.
const ONE_WAY: CallId = 0;

struct Export {
    key: u64,
    /// References sent minus references released.
    live: u64,
}

struct Conn {
    ch: Channel,
    peer: Option<Peer>,
    /// The service this connection came in on, for accepted ones.
    service: Option<String>,
    exports: HashMap<ObjectId, Export>,
    export_ids: HashMap<u64, ObjectId>,
    next_export: ObjectId,
    /// The peer's objects we hold, and how many references to each arrived.
    imports: HashMap<ObjectId, u64>,
    inbox: VecDeque<Message>,
    open: bool,
}

impl Conn {
    fn new(ch: Channel, service: Option<String>) -> Conn {
        Conn {
            peer: ch.peer(),
            ch,
            service,
            exports: HashMap::new(),
            export_ids: HashMap::new(),
            next_export: ROOT + 1,
            imports: HashMap::new(),
            inbox: VecDeque::new(),
            open: true,
        }
    }

    /// Encode `msg` (a call, reply or release) and queue it. A failure means
    /// the connection is unusable and has been marked closed.
    fn send(&mut self, write: impl FnOnce(&mut Encoder)) -> Result<(), CallError> {
        if !self.open {
            return Err(CallError::Disconnected);
        }
        let Conn {
            exports,
            export_ids,
            next_export,
            ..
        } = self;
        let mut export = |key: u64, _interface: &str| -> ObjectId {
            let id = *export_ids.entry(key).or_insert_with(|| {
                let id = *next_export;
                *next_export += 1;
                id
            });
            exports.entry(id).or_insert(Export { key, live: 0 }).live += 1;
            id
        };
        let mut enc = Encoder::new(&mut export);
        write(&mut enc);
        let Encoder { bytes, fds, .. } = enc;
        match self.ch.send(bytes, fds) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::InvalidInput => {
                Err(CallError::Invalid(e.to_string()))
            }
            Err(_) => {
                self.open = false;
                Err(CallError::Disconnected)
            }
        }
    }

    /// Decode every whole message read so far into the inbox.
    fn decode_all(&mut self) -> Result<(), String> {
        while let Some((bytes, fds)) = self.ch.next_message().map_err(|e| e.to_string())? {
            let msg = self.decode(&bytes, fds).map_err(|e| e.to_string())?;
            self.inbox.push_back(msg);
        }
        Ok(())
    }

    fn decode(&mut self, bytes: &[u8], fds: Vec<Handle>) -> Result<Message, DecodeError> {
        let Conn {
            exports, imports, ..
        } = self;
        let mut lookup = |id: ObjectId| exports.get(&id).map(|e| e.key);
        let mut imported = |id: ObjectId| *imports.entry(id).or_insert(0) += 1;
        let mut d = Decoder::new(bytes, fds, &mut lookup, &mut imported);
        let msg = match d.u8()? {
            CALL => Message::Call {
                call: d.u32()?,
                object: d.u32()?,
                method: d.str()?,
                args: d.values()?,
            },
            REPLY_OK => Message::Reply {
                call: d.u32()?,
                result: Ok(d.values()?),
            },
            REPLY_FAULT => Message::Reply {
                call: d.u32()?,
                result: Err(d.str()?),
            },
            RELEASE => Message::Release {
                object: d.u32()?,
                count: d.u64()?,
            },
            other => return Err(DecodeError(format!("unknown message kind {other}"))),
        };
        d.finish()?;
        Ok(msg)
    }
}

struct Listener {
    sys: sys::Listener,
    name: String,
    root: u64,
}

/// Every connection of one thread, and the calls in flight on them.
pub struct Looper {
    conns: BTreeMap<ConnId, Conn>,
    listeners: Vec<Listener>,
    next_conn: ConnId,
    next_call: CallId,
    /// Calls sent and not yet answered, by the connection they went out on.
    pending: HashMap<CallId, ConnId>,
    /// Replies that arrived and have not been collected.
    replies: HashMap<CallId, Result<Vec<Wire>, CallError>>,
    /// Made on first use: setting one up can fail (on Windows), and a
    /// looper that never publishes or connects never needs one.
    poller: Option<sys::Poller>,
}

impl Default for Looper {
    fn default() -> Self {
        Looper::new()
    }
}

impl Looper {
    pub fn new() -> Looper {
        Looper {
            conns: BTreeMap::new(),
            listeners: Vec::new(),
            next_conn: 1,
            next_call: 1,
            pending: HashMap::new(),
            replies: HashMap::new(),
            poller: None,
        }
    }

    fn poller(&mut self) -> io::Result<&mut sys::Poller> {
        if self.poller.is_none() {
            self.poller = Some(sys::Poller::new()?);
        }
        Ok(self.poller.as_mut().expect("just made"))
    }

    // --- Services ----------------------------------------------------------

    /// Publish the object under `root` as service `name`.
    pub fn publish(&mut self, name: &str, root: u64) -> io::Result<()> {
        let listener = sys::Listener::bind(name, self.poller()?)?;
        self.listen(listener, name, root);
        Ok(())
    }

    /// [`Looper::publish`] at an explicit socket path.
    #[cfg(unix)]
    pub fn publish_at(&mut self, path: &std::path::Path, name: &str, root: u64) -> io::Result<()> {
        let listener = sys::Listener::bind_at(path, name)?;
        self.listen(listener, name, root);
        Ok(())
    }

    fn listen(&mut self, listener: sys::Listener, name: &str, root: u64) {
        self.listeners.push(Listener {
            sys: listener,
            name: name.to_owned(),
            root,
        });
    }

    /// Stop accepting clients for `name`. Connections already made stay.
    pub fn unpublish(&mut self, name: &str) {
        self.listeners.retain(|l| l.name != name);
    }

    /// Connect to service `name`. Its root object is [`ROOT`] on the
    /// returned connection.
    pub fn connect(&mut self, name: &str) -> io::Result<ConnId> {
        let ch = sys::connect(name, self.poller()?)?;
        Ok(self.add(ch, None))
    }

    /// [`Looper::connect`] at an explicit socket path.
    #[cfg(unix)]
    pub fn connect_at(&mut self, path: &std::path::Path) -> io::Result<ConnId> {
        let ch = sys::connect_at(path)?;
        Ok(self.add(ch, None))
    }

    /// Take over an already-connected socket (one end of a socket pair
    /// handed to a child process, say).
    #[cfg(unix)]
    pub fn adopt(
        &mut self,
        sock: std::os::unix::net::UnixStream,
        service: Option<String>,
    ) -> io::Result<ConnId> {
        let ch = Channel::new(sock)?;
        Ok(self.add(ch, service))
    }

    fn add(&mut self, ch: Channel, service: Option<String>) -> ConnId {
        let id = self.next_conn;
        self.next_conn += 1;
        self.conns.insert(id, Conn::new(ch, service));
        id
    }

    // --- Connections -------------------------------------------------------

    pub fn is_open(&self, conn: ConnId) -> bool {
        self.conns.get(&conn).is_some_and(|c| c.open)
    }

    /// Who is at the other end of `conn`, as the kernel reports it.
    pub fn peer(&self, conn: ConnId) -> Option<Peer> {
        self.conns.get(&conn).and_then(|c| c.peer)
    }

    /// The service `conn` connected to us through, for incoming ones.
    pub fn service_of(&self, conn: ConnId) -> Option<&str> {
        self.conns.get(&conn).and_then(|c| c.service.as_deref())
    }

    /// Close `conn` now. The handler hears of it at the next dispatch.
    pub fn close(&mut self, conn: ConnId) {
        if let Some(c) = self.conns.get_mut(&conn) {
            let _ = c.ch.flush();
            c.open = false;
        }
    }

    // --- Calls -------------------------------------------------------------

    /// Call `method` on `object` at the other end of `conn` and wait for the
    /// result, serving incoming calls meanwhile.
    pub fn call(
        &mut self,
        handler: &mut dyn Handler,
        conn: ConnId,
        object: ObjectId,
        method: &str,
        args: Vec<Wire>,
    ) -> Result<Vec<Wire>, CallError> {
        let call = self.call_async(conn, object, method, args)?;
        self.wait(handler, call)
    }

    /// Start a call and return without waiting; [`Looper::wait`] or
    /// [`Looper::try_reply`] collects the result.
    pub fn call_async(
        &mut self,
        conn: ConnId,
        object: ObjectId,
        method: &str,
        args: Vec<Wire>,
    ) -> Result<CallId, CallError> {
        let call = self.next_call;
        // Wraps past u32::MAX, skipping the one-way id.
        self.next_call = self.next_call.checked_add(1).unwrap_or(1);
        self.send_call(conn, call, object, method, args)?;
        self.pending.insert(call, conn);
        Ok(call)
    }

    /// A one-way call: sent, never answered.
    pub fn send(
        &mut self,
        conn: ConnId,
        object: ObjectId,
        method: &str,
        args: Vec<Wire>,
    ) -> Result<(), CallError> {
        self.send_call(conn, ONE_WAY, object, method, args)
    }

    fn send_call(
        &mut self,
        conn: ConnId,
        call: CallId,
        object: ObjectId,
        method: &str,
        args: Vec<Wire>,
    ) -> Result<(), CallError> {
        let c = self.conns.get_mut(&conn).ok_or(CallError::Disconnected)?;
        c.send(|e| {
            e.u8(CALL);
            e.u32(call);
            e.u32(object);
            e.str(method);
            e.values(args);
        })
    }

    /// The result of `call` if it has arrived, without waiting.
    pub fn try_reply(&mut self, call: CallId) -> Option<Result<Vec<Wire>, CallError>> {
        self.replies.remove(&call)
    }

    /// Wait for the result of `call`, serving incoming calls meanwhile.
    pub fn wait(
        &mut self,
        handler: &mut dyn Handler,
        call: CallId,
    ) -> Result<Vec<Wire>, CallError> {
        loop {
            if let Some(result) = self.replies.remove(&call) {
                return result;
            }
            if !self.pending.contains_key(&call) {
                return Err(CallError::Invalid(format!("no call {call} is in flight")));
            }
            if let Err(e) = self.dispatch(handler, None) {
                return Err(CallError::Invalid(format!("waiting for a reply: {e}")));
            }
        }
    }

    /// Give back every reference to the peer's object `object` received over
    /// `conn` — the proxy for it has gone.
    pub fn release(&mut self, conn: ConnId, object: ObjectId) {
        let Some(c) = self.conns.get_mut(&conn) else {
            return;
        };
        let Some(count) = c.imports.remove(&object) else {
            return;
        };
        if object != ROOT && count > 0 {
            let _ = c.send(|e| {
                e.u8(RELEASE);
                e.u32(object);
                e.u64(count);
            });
        }
    }

    // --- The loop ----------------------------------------------------------

    /// Handle everything that has arrived, waiting up to `timeout` for
    /// something to (`None`: until something does). Returns how many
    /// messages and new connections were handled.
    pub fn dispatch(
        &mut self,
        handler: &mut dyn Handler,
        timeout: Option<Duration>,
    ) -> io::Result<usize> {
        let mut handled = self.handle_inboxes(handler);
        if handled == 0 {
            self.poll(timeout)?;
            handled += self.accept_all(handler);
            self.read_all();
            handled += self.handle_inboxes(handler);
        }
        self.reap(handler);
        Ok(handled)
    }

    /// Wait until a listener or connection has something, or `timeout`
    /// passes.
    fn poll(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        self.poller()?;
        let Looper {
            listeners,
            conns,
            poller,
            ..
        } = self;
        let ls: Vec<&sys::Listener> = listeners.iter().map(|l| &l.sys).collect();
        let cs: Vec<&Channel> = conns.values().filter(|c| c.open).map(|c| &c.ch).collect();
        poller.as_mut().expect("made above").wait(&ls, &cs, timeout)?;
        // Writing is cheap to retry: flush whatever is queued, ready or not.
        for c in self.conns.values_mut().filter(|c| c.open) {
            if c.ch.wants_write() && c.ch.flush().is_err() {
                c.open = false;
            }
        }
        Ok(())
    }

    fn accept_all(&mut self, handler: &mut dyn Handler) -> usize {
        let mut accepted = Vec::new();
        let Looper {
            listeners, poller, ..
        } = self;
        if let Some(poller) = poller.as_mut() {
            for l in listeners.iter_mut() {
                // A failure to accept one client is that client's problem;
                // the service keeps listening.
                while let Ok(Some(ch)) = l.sys.accept(poller) {
                    accepted.push((ch, l.name.clone(), l.root));
                }
            }
        }
        let n = accepted.len();
        for (ch, name, root) in accepted {
            let id = self.add(ch, Some(name.clone()));
            let c = self.conns.get_mut(&id).expect("just adopted");
            c.exports.insert(ROOT, Export { key: root, live: 1 });
            c.export_ids.insert(root, ROOT);
            handler.connected(self, id, &name);
        }
        n
    }

    fn read_all(&mut self) {
        for c in self.conns.values_mut().filter(|c| c.open) {
            let ok = c.ch.fill().unwrap_or(false);
            // Decode what did arrive even from a peer that then hung up: its
            // last one-way calls still count.
            if c.decode_all().is_err() {
                c.open = false;
                c.inbox.clear();
                continue;
            }
            if !ok {
                c.open = false;
            }
        }
    }

    /// Handle every message waiting in an inbox.
    fn handle_inboxes(&mut self, handler: &mut dyn Handler) -> usize {
        let mut handled = 0;
        loop {
            // One message at a time, re-looking the connection up each time:
            // handling a call can run Saule code that opens or closes
            // connections, or waits and so handles more messages itself.
            let next = self
                .conns
                .iter_mut()
                .find_map(|(&id, c)| c.inbox.pop_front().map(|m| (id, m)));
            let Some((conn, msg)) = next else {
                return handled;
            };
            handled += 1;
            match msg {
                Message::Call {
                    call,
                    object,
                    method,
                    args,
                } => self.handle_call(handler, conn, call, object, &method, args),
                Message::Reply { call, result } => {
                    if self.pending.remove(&call) == Some(conn) {
                        self.replies.insert(call, result.map_err(CallError::Remote));
                    }
                }
                Message::Release { object, count } => self.handle_release(handler, conn, object, count),
            }
        }
    }

    fn handle_call(
        &mut self,
        handler: &mut dyn Handler,
        conn: ConnId,
        call: CallId,
        object: ObjectId,
        method: &str,
        args: Vec<Wire>,
    ) {
        let key = self
            .conns
            .get(&conn)
            .and_then(|c| c.exports.get(&object))
            .map(|e| e.key);
        let result = match key {
            Some(key) => handler.call(self, conn, key, method, args),
            None => Err(format!("no object {object} on this connection")),
        };
        if call == ONE_WAY {
            return;
        }
        let Some(c) = self.conns.get_mut(&conn) else {
            return;
        };
        let sent = match result {
            Ok(values) => c.send(|e| {
                e.u8(REPLY_OK);
                e.u32(call);
                e.values(values);
            }),
            Err(fault) => c.send(|e| {
                e.u8(REPLY_FAULT);
                e.u32(call);
                e.str(&fault);
            }),
        };
        // A result that cannot travel is still an answer the caller is
        // waiting for: send the reason instead.
        if let Err(CallError::Invalid(why)) = sent {
            let _ = c.send(|e| {
                e.u8(REPLY_FAULT);
                e.u32(call);
                e.str(&format!("the result of `{method}` could not be sent: {why}"));
            });
        }
    }

    fn handle_release(&mut self, handler: &mut dyn Handler, conn: ConnId, object: ObjectId, count: u64) {
        if object == ROOT {
            return;
        }
        let Some(c) = self.conns.get_mut(&conn) else {
            return;
        };
        let Some(export) = c.exports.get_mut(&object) else {
            return;
        };
        export.live = export.live.saturating_sub(count);
        if export.live > 0 {
            return;
        }
        let key = export.key;
        c.exports.remove(&object);
        c.export_ids.remove(&key);
        handler.released(self, conn, key);
    }

    /// Drop closed connections: fail the calls waiting on them, release
    /// what they held, and tell the handler.
    fn reap(&mut self, handler: &mut dyn Handler) {
        let closed: Vec<ConnId> = self
            .conns
            .iter()
            .filter(|(_, c)| !c.open && c.inbox.is_empty())
            .map(|(&id, _)| id)
            .collect();
        for id in closed {
            let c = self.conns.remove(&id).expect("listed above");
            let failed: Vec<CallId> = self
                .pending
                .iter()
                .filter(|&(_, &on)| on == id)
                .map(|(&call, _)| call)
                .collect();
            for call in failed {
                self.pending.remove(&call);
                self.replies.insert(call, Err(CallError::Disconnected));
            }
            for export in c.exports.values() {
                handler.released(self, id, export.key);
            }
            handler.disconnected(self, id);
        }
    }

    /// Every open connection.
    pub fn connections(&self) -> impl Iterator<Item = ConnId> + '_ {
        self.conns.iter().filter(|(_, c)| c.open).map(|(&id, _)| id)
    }

    /// The raw descriptors the looper waits on, for a caller that wants to
    /// wait on them together with its own (a display server's input
    /// devices).
    #[cfg(unix)]
    pub fn raw_fds(&self) -> Vec<std::os::fd::RawFd> {
        self.listeners
            .iter()
            .map(|l| l.sys.fd())
            .chain(self.conns.values().filter(|c| c.open).map(|c| c.ch.fd()))
            .collect()
    }

    /// The socket behind `conn`, for tests.
    #[cfg(all(test, unix))]
    pub(crate) fn socket(&self, conn: ConnId) -> Option<&std::os::unix::net::UnixStream> {
        self.conns.get(&conn).map(|c| c.ch.socket())
    }
}
