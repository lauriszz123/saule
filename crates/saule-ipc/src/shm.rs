//! Shared memory: a block of bytes two processes map at once.
//!
//! This is how a window's pixels reach the display server without being
//! copied through a socket — the app draws into the block, the server reads
//! the same block. What travels between them is only the descriptor naming
//! it ([`super::wire::Wire::Shm`]).
//!
//! Linux makes the block with `memfd_create`: anonymous memory with a file
//! descriptor and no name anywhere. macOS has no memfd, so it makes a POSIX
//! shared-memory object under a random name and unlinks the name at once,
//! which leaves the same thing — memory reachable only through the
//! descriptor. Windows makes an unnamed section with `CreateFileMapping`,
//! reachable only through its handle. (See `sys`.)

use std::io;
use std::ptr::NonNull;

use super::sys::{self, Handle};

/// A mapped block of shared memory.
pub struct SharedMemory {
    handle: Handle,
    ptr: NonNull<u8>,
    len: usize,
}

impl SharedMemory {
    /// A new block of `len` bytes, zeroed.
    pub fn create(len: usize) -> io::Result<SharedMemory> {
        if len == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "shared memory cannot be empty",
            ));
        }
        let handle = sys::shm_create(len)?;
        SharedMemory::map(handle, len)
    }

    /// Map a block another process made and handed over, `len` bytes of it.
    /// Fails when the block is not at least that large, so a peer cannot
    /// make us read past the end of what it really shares.
    pub fn from_fd(handle: Handle, len: usize) -> io::Result<SharedMemory> {
        if len == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "shared memory cannot be empty",
            ));
        }
        SharedMemory::map(handle, len)
    }

    fn map(handle: Handle, len: usize) -> io::Result<SharedMemory> {
        let ptr = sys::shm_map(&handle, len)?;
        Ok(SharedMemory { handle, ptr, len })
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The bytes. Another process may be changing them as they are read;
    /// what they mean, and when it is safe to look, is for the protocol on
    /// top to say (a window is read after its `commit`).
    pub fn as_slice(&self) -> &[u8] {
        // Safety: `len` mapped bytes, alive as long as `self`.
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        // Safety: as above, and `&mut self` keeps this process's own
        // accesses exclusive.
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }

    /// Another handle to the same block, to hand to another process.
    pub fn share(&self) -> io::Result<Handle> {
        self.handle.try_clone()
    }
}

impl Drop for SharedMemory {
    fn drop(&mut self) {
        sys::shm_unmap(self.ptr, self.len);
    }
}

impl std::fmt::Debug for SharedMemory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SharedMemory({} bytes)", self.len)
    }
}
