//! Finding services by name, and knowing who is on the other end.
//!
//! There is no broker: a service is reachable the moment it is published and
//! gone the moment its process is.
//!
//! - **Unix**: a published service is a socket in the **services
//!   directory**, named after the service — `saule.display` is
//!   `<dir>/saule.display`. The directory is created mode 0700, so only this
//!   user can publish or connect.
//! - **Windows**: it is a named pipe, `\\.\pipe\saule-<space>-<name>`,
//!   where `<space>` stands for the services directory (see [`pipe_name`]).
//!   The pipe takes local clients only, and its default security admits only
//!   this user to write.
//!
//! The services directory, first that applies:
//! 1. `SAULE_SERVICES_DIR`, when set;
//! 2. `$XDG_RUNTIME_DIR/saule/services`, a desktop session's per-user dir;
//! 3. `/run/saule/services` for root — Saule Linux, where everything runs as
//!    root and init has mounted `/run`;
//! 4. `<temp>/saule-<user>/services` otherwise (macOS, Windows).

use std::io;
use std::path::{Path, PathBuf};

/// The directory services are published in (see the module docs).
pub fn services_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("SAULE_SERVICES_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
        return Path::new(&dir).join("saule").join("services");
    }
    #[cfg(unix)]
    let user = {
        // Safety: no arguments, cannot fail.
        let uid = unsafe { libc::getuid() };
        if uid == 0 && Path::new("/run").is_dir() {
            return PathBuf::from("/run/saule/services");
        }
        uid.to_string()
    };
    #[cfg(not(unix))]
    let user = std::env::var("USERNAME").unwrap_or_default();
    std::env::temp_dir()
        .join(format!("saule-{user}"))
        .join("services")
}

/// Whether `name` can name a service: lowercase letters, digits, `.`, `-`
/// and `_`, starting with a letter, at most 64 bytes. Reverse-domain names
/// like `saule.display` are the convention.
pub fn valid_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z'))
        && name.len() <= 64
        && bytes.all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-' | b'_'))
}

/// The socket path of service `name`.
#[cfg(unix)]
pub(crate) fn path_for(name: &str) -> io::Result<PathBuf> {
    check_name(name)?;
    Ok(services_dir().join(name))
}

fn check_name(name: &str) -> io::Result<()> {
    if !valid_name(name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "`{name}` is not a service name: use lowercase letters, digits, `.`, `-` and `_`, starting with a letter"
            ),
        ));
    }
    Ok(())
}

/// Who is at the other end of a connection, as the kernel vouches for it —
/// not what the peer says about itself.
///
/// Windows has no numeric user ids: `uid` and `gid` are 0 there, and a
/// service is reachable only by its own user anyway (see `sys::windows`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peer {
    pub pid: Option<u32>,
    pub uid: u32,
    pub gid: u32,
}

/// The pipe name of service `name`: `\\.\pipe\saule-<space>-<name>`, where
/// `<space>` is a hash of the services directory — so two independent setups
/// (a test with its own `SAULE_SERVICES_DIR`, say) never meet, as two
/// directories would not on Unix.
#[cfg(windows)]
pub(crate) fn pipe_name(name: &str) -> io::Result<String> {
    check_name(name)?;
    // FNV-1a: stable across runs and builds, unlike `DefaultHasher`.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in services_dir().to_string_lossy().bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    Ok(format!(r"\\.\pipe\saule-{h:016x}-{name}"))
}

