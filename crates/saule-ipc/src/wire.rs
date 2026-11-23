//! The values that cross a process boundary, and their encoding.
//!
//! Everything an `extern interface` method takes or returns becomes a
//! [`Wire`] on the way out and comes back from one on the way in. The set is
//! deliberately small — what two programs can share without sharing memory:
//!
//! | Wire | Saule |
//! |---|---|
//! | `Nil`, `Bool`, `Int`, `Float`, `Str`, `Bytes` | the scalar of the same name |
//! | `List`, `Map` | a table, by its array and its keyed part |
//! | `Record` | a plain data class, copied field by field |
//! | `Object` | an `extern interface` value, passed **by reference** |
//! | `Fd` | an open file, socket or device, handed over |
//! | `Shm` | shared memory: the same bytes, mapped by both sides |
//!
//! ## Objects
//!
//! An object is never copied; what travels is a number naming it on the
//! connection, and the receiver gets a proxy. Which side the number belongs to
//! is part of the encoding, because both sides hand out numbers: an app
//! passing the window manager its own `WindowEvents` sends one of *its*
//! numbers, while passing back a `Window` the manager gave it sends one of
//! the *manager's*. [`ObjRef`] is written from the point of view of whoever
//! holds the value; the encoding flips it, so a decoded reference is again
//! from the receiver's point of view.
//!
//! ## Format
//!
//! A tag byte, then the value, little-endian throughout. Strings and byte
//! strings carry a `u32` length. File descriptors are not in the bytes at all
//! — the socket carries them alongside the message — so `Fd` and `Shm` hold
//! an index into the message's descriptor list.

use super::sys::Handle;

/// A value on its way to or from another process.
#[derive(Debug)]
pub enum Wire {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
    /// A table's array part.
    List(Vec<Wire>),
    /// A table's keyed part, in no particular order.
    Map(Vec<(Wire, Wire)>),
    /// An instance of a plain data class: its class name and its fields by
    /// name, so two builds that order the fields differently still agree.
    Record {
        class: String,
        fields: Vec<(String, Wire)>,
    },
    /// An `extern interface` value, by reference.
    Object(ObjRef),
    /// An open file, socket or device, handed over: a file descriptor on
    /// Unix, a handle on Windows.
    Fd(Handle),
    /// Shared memory: a handle both sides map, and its length.
    Shm(Handle, u64),
}

/// An object reference, from the point of view of the side holding it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObjRef {
    /// An object living in this process, named by the key its owner (the
    /// layer above the looper) gave it. Sent, it becomes an id of ours.
    Local { key: u64, interface: String },
    /// An object living in the process at the other end of the connection,
    /// by the id that process gave it. Sent back, it is one of *their* ids.
    Remote { id: u32, interface: String },
}

/// Caps on what a peer can make us allocate or recurse into.
pub const MAX_DEPTH: usize = 64;

const NIL: u8 = 0;
const FALSE: u8 = 1;
const TRUE: u8 = 2;
const INT: u8 = 3;
const FLOAT: u8 = 4;
const STR: u8 = 5;
const BYTES: u8 = 6;
const LIST: u8 = 7;
const MAP: u8 = 8;
const RECORD: u8 = 9;
/// An object the *sender* owns.
const OBJ_SENDERS: u8 = 10;
/// An object the *receiver* owns, being handed back to it.
const OBJ_RECEIVERS: u8 = 11;
const FD: u8 = 12;
const SHM: u8 = 13;

/// Turns values into bytes plus a list of descriptors.
pub struct Encoder<'a> {
    pub(crate) bytes: Vec<u8>,
    pub(crate) fds: Vec<Handle>,
    /// Gives a local object its id on this connection, exporting it if this
    /// is the first time it is sent.
    export: &'a mut dyn FnMut(u64, &str) -> u32,
}

impl<'a> Encoder<'a> {
    pub(crate) fn new(export: &'a mut dyn FnMut(u64, &str) -> u32) -> Encoder<'a> {
        Encoder {
            bytes: Vec::new(),
            fds: Vec::new(),
            export,
        }
    }

    pub(crate) fn u8(&mut self, v: u8) {
        self.bytes.push(v);
    }

    pub(crate) fn u32(&mut self, v: u32) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    pub(crate) fn u64(&mut self, v: u64) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    pub(crate) fn str(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.bytes.extend_from_slice(s.as_bytes());
    }

    pub(crate) fn values(&mut self, values: Vec<Wire>) {
        self.u32(values.len() as u32);
        for v in values {
            self.value(v);
        }
    }

    pub(crate) fn value(&mut self, v: Wire) {
        match v {
            Wire::Nil => self.u8(NIL),
            Wire::Bool(false) => self.u8(FALSE),
            Wire::Bool(true) => self.u8(TRUE),
            Wire::Int(i) => {
                self.u8(INT);
                self.u64(i as u64);
            }
            Wire::Float(f) => {
                self.u8(FLOAT);
                self.u64(f.to_bits());
            }
            Wire::Str(s) => {
                self.u8(STR);
                self.str(&s);
            }
            Wire::Bytes(b) => {
                self.u8(BYTES);
                self.u32(b.len() as u32);
                self.bytes.extend_from_slice(&b);
            }
            Wire::List(items) => {
                self.u8(LIST);
                self.values(items);
            }
            Wire::Map(pairs) => {
                self.u8(MAP);
                self.u32(pairs.len() as u32);
                for (k, v) in pairs {
                    self.value(k);
                    self.value(v);
                }
            }
            Wire::Record { class, fields } => {
                self.u8(RECORD);
                self.str(&class);
                self.u32(fields.len() as u32);
                for (name, v) in fields {
                    self.str(&name);
                    self.value(v);
                }
            }
            Wire::Object(ObjRef::Local { key, interface }) => {
                let id = (self.export)(key, &interface);
                self.u8(OBJ_SENDERS);
                self.u32(id);
                self.str(&interface);
            }
            Wire::Object(ObjRef::Remote { id, interface }) => {
                self.u8(OBJ_RECEIVERS);
                self.u32(id);
                self.str(&interface);
            }
            Wire::Fd(fd) => {
                self.u8(FD);
                self.u32(self.fds.len() as u32);
                self.fds.push(fd);
            }
            Wire::Shm(fd, len) => {
                self.u8(SHM);
                self.u32(self.fds.len() as u32);
                self.u64(len);
                self.fds.push(fd);
            }
        }
    }
}

/// Why a message could not be read. Any of these means the peer is broken or
/// hostile, and the connection is closed.
#[derive(Debug, PartialEq, Eq)]
pub struct DecodeError(pub String);

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "malformed message: {}", self.0)
    }
}

/// Reads values back out of a message's bytes and descriptors.
pub struct Decoder<'a> {
    bytes: &'a [u8],
    at: usize,
    fds: Vec<Option<Handle>>,
    /// Turns one of our own object ids back into the key it was exported
    /// for. `None` for an id we never handed out on this connection.
    lookup: &'a mut dyn FnMut(u32) -> Option<u64>,
    /// Records that a reference to one of the peer's objects arrived, for
    /// the release count (see `looper`).
    imported: &'a mut dyn FnMut(u32),
}

impl<'a> Decoder<'a> {
    pub(crate) fn new(
        bytes: &'a [u8],
        fds: Vec<Handle>,
        lookup: &'a mut dyn FnMut(u32) -> Option<u64>,
        imported: &'a mut dyn FnMut(u32),
    ) -> Decoder<'a> {
        Decoder {
            bytes,
            at: 0,
            fds: fds.into_iter().map(Some).collect(),
            lookup,
            imported,
        }
    }

    fn fail<T>(&self, what: &str) -> Result<T, DecodeError> {
        Err(DecodeError(format!("{what} at byte {}", self.at)))
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if self.bytes.len() - self.at < n {
            return self.fail("message ends early");
        }
        let out = &self.bytes[self.at..self.at + n];
        self.at += n;
        Ok(out)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub(crate) fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    pub(crate) fn str(&mut self) -> Result<String, DecodeError> {
        let n = self.u32()? as usize;
        let raw = self.take(n)?;
        match std::str::from_utf8(raw) {
            Ok(s) => Ok(s.to_owned()),
            Err(_) => self.fail("a string that is not UTF-8"),
        }
    }

    /// A count of things still to come, checked against the bytes left so a
    /// forged count cannot make us reserve gigabytes.
    fn count(&mut self, min_size: usize) -> Result<usize, DecodeError> {
        let n = self.u32()? as usize;
        if n.saturating_mul(min_size) > self.bytes.len() - self.at {
            return self.fail("a count larger than the message");
        }
        Ok(n)
    }

    pub(crate) fn values(&mut self) -> Result<Vec<Wire>, DecodeError> {
        let n = self.count(1)?;
        (0..n).map(|_| self.value(0)).collect()
    }

    fn fd(&mut self) -> Result<Handle, DecodeError> {
        let i = self.u32()? as usize;
        match self.fds.get_mut(i).and_then(Option::take) {
            Some(fd) => Ok(fd),
            None => self.fail("a descriptor the message does not carry"),
        }
    }

    pub(crate) fn value(&mut self, depth: usize) -> Result<Wire, DecodeError> {
        if depth > MAX_DEPTH {
            return self.fail("values nested too deeply");
        }
        Ok(match self.u8()? {
            NIL => Wire::Nil,
            FALSE => Wire::Bool(false),
            TRUE => Wire::Bool(true),
            INT => Wire::Int(self.u64()? as i64),
            FLOAT => Wire::Float(f64::from_bits(self.u64()?)),
            STR => Wire::Str(self.str()?),
            BYTES => {
                let n = self.u32()? as usize;
                Wire::Bytes(self.take(n)?.to_vec())
            }
            LIST => {
                let n = self.count(1)?;
                let mut items = Vec::with_capacity(n);
                for _ in 0..n {
                    items.push(self.value(depth + 1)?);
                }
                Wire::List(items)
            }
            MAP => {
                let n = self.count(2)?;
                let mut pairs = Vec::with_capacity(n);
                for _ in 0..n {
                    let k = self.value(depth + 1)?;
                    let v = self.value(depth + 1)?;
                    pairs.push((k, v));
                }
                Wire::Map(pairs)
            }
            RECORD => {
                let class = self.str()?;
                let n = self.count(5)?;
                let mut fields = Vec::with_capacity(n);
                for _ in 0..n {
                    let name = self.str()?;
                    fields.push((name, self.value(depth + 1)?));
                }
                Wire::Record { class, fields }
            }
            OBJ_SENDERS => {
                let id = self.u32()?;
                let interface = self.str()?;
                (self.imported)(id);
                Wire::Object(ObjRef::Remote { id, interface })
            }
            OBJ_RECEIVERS => {
                let id = self.u32()?;
                let interface = self.str()?;
                match (self.lookup)(id) {
                    Some(key) => Wire::Object(ObjRef::Local { key, interface }),
                    None => return self.fail("a reference to an object never exported"),
                }
            }
            FD => Wire::Fd(self.fd()?),
            SHM => {
                let fd = self.fd()?;
                Wire::Shm(fd, self.u64()?)
            }
            other => return self.fail(&format!("unknown value tag {other}")),
        })
    }

    pub(crate) fn finish(&self) -> Result<(), DecodeError> {
        if self.at != self.bytes.len() {
            return self.fail("bytes after the end of the message");
        }
        Ok(())
    }
}
