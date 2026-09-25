//! Reading a package's metadata out of its library file.
//!
//! The records are exported statics (see `saule_native_abi`, "Package
//! metadata"), so finding them is a walk of the export table, and reading
//! one is a lookup of the bytes behind an address. Nothing here loads the
//! library or runs any of its code: the file is parsed as data, exactly as
//! a text manifest was.

use object::{Object, ObjectSection};
use saule_native_abi::{META_HEADER_LEN, META_MAGIC, META_SYMBOL_PREFIX};

/// One metadata record: the symbol suffix it was exported under (`PACKAGE`,
/// `C_Image`, …) and its TOML payload.
pub(crate) struct RawRecord {
    pub(crate) symbol: String,
    pub(crate) payload: String,
}

/// Every metadata record in `bytes`, the contents of a library file.
///
/// An empty result is not an error here — it is a shared library that is
/// not a Saule package, and the caller decides what to say about that.
pub(crate) fn read_records(bytes: &[u8]) -> Result<Vec<RawRecord>, String> {
    let file = object::File::parse(bytes)
        .map_err(|e| format!("is not a shared library this platform can read ({e})"))?;
    let exports = file
        .exports()
        .map_err(|e| format!("has an unreadable export table ({e})"))?;

    let mut out = Vec::new();
    for export in exports {
        let Ok(name) = std::str::from_utf8(export.name()) else {
            continue;
        };
        // Mach-O prefixes every C symbol with an underscore.
        let name = name.strip_prefix('_').unwrap_or(name);
        let Some(suffix) = name.strip_prefix(META_SYMBOL_PREFIX) else {
            continue;
        };
        let payload = read_record_at(&file, export.address())
            .map_err(|e| format!("has a corrupt metadata record `{name}`: {e}"))?;
        out.push(RawRecord {
            symbol: suffix.to_string(),
            payload,
        });
    }
    Ok(out)
}

/// Read the record whose first byte is at virtual address `address`.
fn read_record_at(file: &object::File<'_>, address: u64) -> Result<String, String> {
    let header = bytes_at(file, address, META_HEADER_LEN as u64)?;
    if header[..META_MAGIC.len()] != META_MAGIC {
        return Err(format!(
            "it does not start with `{}` — it was written by an SDK whose record \
             format this toolchain does not read",
            String::from_utf8_lossy(&META_MAGIC)
        ));
    }
    let len_bytes: [u8; 4] = header[META_MAGIC.len()..]
        .try_into()
        .expect("the header is exactly magic + u32");
    let len = u32::from_le_bytes(len_bytes);
    let payload = bytes_at(file, address + META_HEADER_LEN as u64, u64::from(len))?;
    String::from_utf8(payload.to_vec()).map_err(|_| "its payload is not UTF-8".to_string())
}

/// The `len` bytes of file data behind virtual address `address`.
fn bytes_at<'d>(file: &object::File<'d>, address: u64, len: u64) -> Result<&'d [u8], String> {
    for section in file.sections() {
        let start = section.address();
        let end = start.saturating_add(section.size());
        if address < start || address >= end {
            continue;
        }
        return match section.data_range(address, len) {
            Ok(Some(bytes)) => Ok(bytes),
            Ok(None) => Err(format!(
                "{len} bytes at {address:#x} run past the end of their section"
            )),
            Err(e) => Err(format!("its section could not be read ({e})")),
        };
    }
    Err(format!("no section holds address {address:#x}"))
}
