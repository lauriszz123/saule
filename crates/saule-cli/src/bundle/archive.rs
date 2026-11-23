//! The payload `saule build` appends to a copy of the saule binary.
//!
//! ```text
//! ┌──────────────────────┬────────────────────────┬────────────────────┐
//! │ saule (the runner)   │ archive                │ footer (32 bytes)  │
//! └──────────────────────┴────────────────────────┴────────────────────┘
//!
//! archive = u32 count, then per entry: u32 path length, path (UTF-8, `/`
//!           separated), u64 data length, data
//! footer  = "SAULEAPP"  u32 format  u32 reserved  u64 archive length
//!           u64 archive hash
//! ```
//!
//! The footer sits at the very end so the runner finds it with one seek,
//! without knowing how long the runner part is. All integers are
//! little-endian. The hash (FNV-1a, not a security measure) names the cache
//! directory the archive unpacks into, so two builds never share one.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub const MAGIC: &[u8; 8] = b"SAULEAPP";
pub const FORMAT: u32 = 1;
pub const FOOTER_LEN: u64 = 32;

/// Paths inside the archive.
pub const MANIFEST: &str = "app.manifest";
pub const PROGRAM: &str = "program.sbc";
pub const FILES: &str = "files/";
pub const NATIVE: &str = "native_packages/";

pub struct Entry {
    pub path: String,
    pub data: Vec<u8>,
}

/// The archive and where it starts in the executable.
pub struct Embedded {
    pub entries: Vec<Entry>,
    pub hash: u64,
}

impl Embedded {
    pub fn get(&self, path: &str) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|e| e.path == path)
            .map(|e| e.data.as_slice())
    }
}

pub fn pack(entries: &[Entry]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for entry in entries {
        out.extend_from_slice(&(entry.path.len() as u32).to_le_bytes());
        out.extend_from_slice(entry.path.as_bytes());
        out.extend_from_slice(&(entry.data.len() as u64).to_le_bytes());
        out.extend_from_slice(&entry.data);
    }
    let hash = fnv1a(&out);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&((out.len() - 16) as u64).to_le_bytes());
    out.extend_from_slice(&hash.to_le_bytes());
    out
}

/// Where the runner part of `exe` ends: its whole length, unless it already
/// carries an archive (building with a built app's binary).
pub fn runner_len(exe: &Path) -> std::io::Result<u64> {
    let mut file = std::fs::File::open(exe)?;
    let len = file.metadata()?.len();
    Ok(match footer(&mut file, len)? {
        Some((archive_len, _)) => len - FOOTER_LEN - archive_len,
        None => len,
    })
}

/// The archive appended to `exe`, if there is one.
pub fn read(exe: &Path) -> Result<Option<Embedded>, String> {
    let mut file = std::fs::File::open(exe).map_err(|e| e.to_string())?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    let Some((archive_len, hash)) = footer(&mut file, len).map_err(|e| e.to_string())? else {
        return Ok(None);
    };

    let start = len - FOOTER_LEN - archive_len;
    file.seek(SeekFrom::Start(start))
        .map_err(|e| e.to_string())?;
    let mut bytes = vec![0; archive_len as usize];
    file.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    if fnv1a(&bytes) != hash {
        return Err("the program packed into this executable is damaged".into());
    }

    let entries = unpack(&bytes).ok_or("the program packed into this executable is damaged")?;
    Ok(Some(Embedded { entries, hash }))
}

/// `(archive length, hash)` from the footer, or `None` for a plain binary.
fn footer(file: &mut std::fs::File, len: u64) -> std::io::Result<Option<(u64, u64)>> {
    if len < FOOTER_LEN {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(len - FOOTER_LEN))?;
    let mut f = [0u8; FOOTER_LEN as usize];
    file.read_exact(&mut f)?;
    if &f[..8] != MAGIC {
        return Ok(None);
    }
    let format = u32::from_le_bytes(f[8..12].try_into().expect("4 bytes"));
    let archive_len = u64::from_le_bytes(f[16..24].try_into().expect("8 bytes"));
    let hash = u64::from_le_bytes(f[24..32].try_into().expect("8 bytes"));
    if format != FORMAT || archive_len > len - FOOTER_LEN {
        return Ok(None);
    }
    Ok(Some((archive_len, hash)))
}

fn unpack(bytes: &[u8]) -> Option<Vec<Entry>> {
    let mut at = 0usize;
    let mut take = |n: usize| -> Option<&[u8]> {
        let out = bytes.get(at..at.checked_add(n)?)?;
        at += n;
        Some(out)
    };
    let count = u32::from_le_bytes(take(4)?.try_into().ok()?);
    let mut entries = Vec::new();
    for _ in 0..count {
        let path_len = u32::from_le_bytes(take(4)?.try_into().ok()?) as usize;
        let path = String::from_utf8(take(path_len)?.to_vec()).ok()?;
        let data_len = u64::from_le_bytes(take(8)?.try_into().ok()?) as usize;
        let data = take(data_len)?.to_vec();
        entries.push(Entry { path, data });
    }
    Some(entries)
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_archive_appended_to_a_binary_reads_back() {
        let dir = std::env::temp_dir().join(format!("saule-archive-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("app");

        let entries = vec![
            Entry {
                path: PROGRAM.into(),
                data: b"bytecode".to_vec(),
            },
            Entry {
                path: format!("{FILES}a/b.txt"),
                data: vec![],
            },
        ];
        let mut bytes = b"pretend this is saule".to_vec();
        let runner = bytes.len() as u64;
        bytes.extend(pack(&entries));
        std::fs::write(&exe, &bytes).unwrap();

        let embedded = read(&exe).unwrap().expect("found the archive");
        assert_eq!(embedded.get(PROGRAM), Some(&b"bytecode"[..]));
        assert_eq!(embedded.get("files/a/b.txt"), Some(&b""[..]));
        assert_eq!(runner_len(&exe).unwrap(), runner);

        // One flipped byte in the archive is caught.
        bytes[runner as usize + 10] ^= 1;
        std::fs::write(&exe, &bytes).unwrap();
        assert!(read(&exe).is_err());

        std::fs::write(&exe, b"just a binary").unwrap();
        assert!(read(&exe).unwrap().is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
