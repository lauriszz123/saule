//! Making a freshly built macOS library loadable.
//!
//! Apple's linker (`ld-27037.1`, Xcode 26) writes the symbol string table
//! straight after the indirect symbol table, so an **odd** number of 4-byte
//! indirect entries leaves `LC_SYMTAB.stroff` 4-byte aligned. macOS 27's dyld
//! requires 8 and refuses the file outright:
//!
//! ```text
//! mis-aligned LINKEDIT string pool, fileOffset=0x000FAEE4
//! ```
//!
//! It pads correctly in some builds and not others, so whether a link comes
//! out loadable turns on a symbol count that changes with any edit — which is
//! why a package is repaired after it is built rather than nudged with a build
//! flag. The repair inserts the padding and moves every following `__LINKEDIT`
//! region along with it.
//!
//! Only this platform has the problem — ELF and PE have no equivalent — so
//! the module is compiled only here, and `install` calls it only here.
//! `scripts/align_macho_strtab.py` in this repository is the same repair for a
//! library built outside `saule install`.

use std::path::Path;
use std::process::Command;

const MH_MAGIC_64: u32 = 0xFEED_FACF;
const FAT_MAGIC: u32 = 0xCAFE_BABE;
const FAT_CIGAM: u32 = 0xBEBA_FECA;

const LC_SEGMENT_64: u32 = 0x19;
const LC_SYMTAB: u32 = 0x02;
const LC_DYSYMTAB: u32 = 0x0B;

/// Commands whose payload is a `linkedit_data_command`: `cmd`, `cmdsize`,
/// `dataoff`, `datasize`. Their data all lives in `__LINKEDIT`.
const LINKEDIT_DATA_CMDS: &[u32] = &[
    0x1D,        // LC_CODE_SIGNATURE
    0x1E,        // LC_SEGMENT_SPLIT_INFO
    0x26,        // LC_FUNCTION_STARTS
    0x29,        // LC_DATA_IN_CODE
    0x2B,        // LC_DYLIB_CODE_SIGN_DRS
    0x2E,        // LC_LINKER_OPTIMIZATION_HINT
    0x8000_0033, // LC_DYLD_EXPORTS_TRIE
    0x8000_0034, // LC_DYLD_CHAINED_FIXUPS
];

/// `LC_DYLD_INFO` / `LC_DYLD_INFO_ONLY`: five `(offset, size)` pairs.
const DYLD_INFO_CMDS: &[u32] = &[0x22, 0x8000_0022];

/// Byte offsets of the `__LINKEDIT` file offsets within `LC_DYSYMTAB`.
/// Spelled out rather than strided: the command interleaves offsets with
/// counts, so a loop over a stride reads the wrong fields.
const DYSYMTAB_OFFSETS: &[usize] = &[
    32, // tocoff
    40, // modtaboff
    48, // extrefsymoff
    56, // indirectsymoff
    64, // extreloff
    72, // locreloff
];

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(data: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(data.get(at..at + 8)?.try_into().ok()?))
}

fn put_u32(data: &mut [u8], at: usize, value: u32) {
    data[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u64(data: &mut [u8], at: usize, value: u64) {
    data[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

/// Whether `data` is a 64-bit Mach-O whose string pool is misaligned.
///
/// Anything this must not touch — an ELF binary, a script, a universal
/// binary — answers `false`: there is nothing here to repair.
pub(crate) fn needs_padding(data: &[u8]) -> bool {
    stroff(data).is_some_and(|(_, value)| value % 8 != 0)
}

/// The location and value of `LC_SYMTAB.stroff`.
fn stroff(data: &[u8]) -> Option<(usize, u32)> {
    if data.len() < 32 {
        return None;
    }
    let magic = u32::from_be_bytes(data[..4].try_into().ok()?);
    if magic == FAT_MAGIC || magic == FAT_CIGAM {
        // A universal binary holds several Mach-Os; each would need
        // repairing and the fat header rewriting. Nothing here produces one.
        return None;
    }
    if u32_at(data, 0)? != MH_MAGIC_64 {
        return None;
    }
    let ncmds = u32_at(data, 16)?;
    let mut pos = 32usize;
    for _ in 0..ncmds {
        let cmd = u32_at(data, pos)?;
        let cmdsize = u32_at(data, pos + 4)? as usize;
        if cmdsize == 0 {
            return None;
        }
        if cmd == LC_SYMTAB {
            return Some((pos + 16, u32_at(data, pos + 16)?));
        }
        pos += cmdsize;
    }
    None
}

/// Pad `path`'s string pool to an 8-byte boundary, in place.
///
/// Returns how many bytes were inserted — `0` when the file was already fine
/// or is not something to repair, so this is safe to call on anything.
pub(crate) fn align_string_pool(path: &Path) -> Result<usize, String> {
    let read = || std::fs::read(path).map_err(|e| format!("reading {}: {e}", path.display()));
    if !needs_padding(&read()?) {
        return Ok(0);
    }

    // The ad-hoc signature covers the bytes about to move, and `codesign`
    // will not replace a signature in a file it reads as inconsistent — so
    // it comes off first, with Apple's own tool, and goes back on after.
    run("codesign", &["--remove-signature", &path.to_string_lossy()])?;

    let mut data = read()?;
    let (stroff_at, old_stroff) =
        stroff(&data).ok_or_else(|| format!("{}: no LC_SYMTAB", path.display()))?;
    let pad = ((8 - (old_stroff as usize % 8)) % 8) as u32;
    if pad == 0 {
        return Ok(0);
    }

    // Collect every field holding a `__LINKEDIT` file offset, so they move
    // together, plus the segment command that has to grow.
    let mut offsets: Vec<usize> = vec![stroff_at];
    let mut linkedit: Option<usize> = None;
    let ncmds = u32_at(&data, 16).ok_or("truncated header")?;
    let mut pos = 32usize;
    for _ in 0..ncmds {
        let cmd = u32_at(&data, pos).ok_or("truncated load command")?;
        let cmdsize = u32_at(&data, pos + 4).ok_or("truncated load command")? as usize;
        if cmdsize == 0 {
            return Err(format!("{}: a load command has size 0", path.display()));
        }
        if cmd == LC_SEGMENT_64 {
            let name = &data[pos + 8..pos + 24];
            if name.starts_with(b"__LINKEDIT\0") {
                linkedit = Some(pos);
            }
        } else if cmd == LC_SYMTAB {
            offsets.push(pos + 8); // symoff
        } else if cmd == LC_DYSYMTAB {
            offsets.extend(DYSYMTAB_OFFSETS.iter().map(|o| pos + o));
        } else if LINKEDIT_DATA_CMDS.contains(&cmd) {
            offsets.push(pos + 8); // dataoff
        } else if DYLD_INFO_CMDS.contains(&cmd) {
            offsets.extend((0..5).map(|i| pos + 8 + i * 8));
        }
        pos += cmdsize;
    }
    let seg = linkedit.ok_or_else(|| format!("{}: no __LINKEDIT segment", path.display()))?;

    // Insert the padding, then move everything at or after the old string
    // table. A zero offset means "no such table" and must stay zero.
    data.splice(
        old_stroff as usize..old_stroff as usize,
        std::iter::repeat_n(0u8, pad as usize),
    );
    for at in offsets {
        let Some(value) = u32_at(&data, at) else {
            continue;
        };
        if value != 0 && value >= old_stroff {
            put_u32(&mut data, at, value + pad);
        }
    }

    // `__LINKEDIT` grew. Its vmsize is page-rounded and must still cover it.
    let filesize = u64_at(&data, seg + 48).ok_or("truncated segment")? + u64::from(pad);
    put_u64(&mut data, seg + 48, filesize);
    let vmsize = u64_at(&data, seg + 32).ok_or("truncated segment")?;
    let page = 0x4000u64;
    let needed = filesize.div_ceil(page) * page;
    if vmsize < needed {
        put_u64(&mut data, seg + 32, needed);
    }

    std::fs::write(path, &data).map_err(|e| format!("writing {}: {e}", path.display()))?;
    run(
        "codesign",
        &["--force", "--sign", "-", &path.to_string_lossy()],
    )?;

    // Check the result rather than trust the arithmetic: this rewrites a
    // Mach-O by hand, and a file wrong in some other way would otherwise
    // reach the user as a cryptic dyld error.
    if needs_padding(&read()?) {
        return Err(format!(
            "{}: padding did not align the string pool",
            path.display()
        ));
    }
    run("codesign", &["--verify", &path.to_string_lossy()])?;
    Ok(pad as usize)
}

fn run(program: &str, args: &[&str]) -> Result<(), String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("could not run `{program}`: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    Err(format!("`{program}` failed: {}", err.trim()))
}
