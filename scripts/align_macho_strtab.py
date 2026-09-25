#!/usr/bin/env python3
"""Pad a Mach-O file so its symbol string table starts 8-byte aligned.

macOS 27's dyld refuses to load a Mach-O whose `LC_SYMTAB.stroff` is not a
multiple of 8:

    mis-aligned LINKEDIT string pool, fileOffset=0x000FAEE4

Apple's linker (`ld-27037.1`) writes the string table immediately after the
indirect symbol table, and an indirect symbol table with an **odd** number of
4-byte entries leaves it 4-byte aligned. It pads correctly in some builds and
not in others, so whether a link comes out loadable depends on a symbol count
that changes with any edit — which is why this is repaired after the fact
rather than nudged with a build flag.

The repair inserts padding before the string table and moves every following
`__LINKEDIT` region along with it. A file that is already aligned is left
untouched, so this is safe to run unconditionally and on any platform.

    align_macho_strtab.py [-v] <file>...        repair in place
    align_macho_strtab.py --check [-v] <file>... report, change nothing

Exits non-zero if a file could not be repaired, or under `--check` if one is
misaligned. A file that needs no change, or that is not a Mach-O at all (an ELF
binary, a script), is passed over silently unless `-v` is given, so this is safe
to run over a whole directory on any platform.
"""

import shutil
import struct
import subprocess
import sys

MH_MAGIC_64 = 0xFEEDFACF
MH_CIGAM_64 = 0xCFFAEDFE
FAT_MAGIC = 0xCAFEBABE
FAT_CIGAM = 0xBEBAFECA

LC_SEGMENT_64 = 0x19
LC_SYMTAB = 0x02
LC_DYSYMTAB = 0x0B
LC_CODE_SIGNATURE = 0x1D
# Every command whose payload is a `linkedit_data_command`: cmd, cmdsize,
# dataoff, datasize. Their data all lives in __LINKEDIT.
LINKEDIT_DATA_CMDS = {
    0x1D,  # LC_CODE_SIGNATURE
    0x1E,  # LC_SEGMENT_SPLIT_INFO
    0x26,  # LC_FUNCTION_STARTS
    0x29,  # LC_DATA_IN_CODE
    0x2B,  # LC_DYLIB_CODE_SIGN_DRS
    0x2E,  # LC_LINKER_OPTIMIZATION_HINT
    0x80000033,  # LC_DYLD_EXPORTS_TRIE
    0x80000034,  # LC_DYLD_CHAINED_FIXUPS
}
# LC_DYLD_INFO / LC_DYLD_INFO_ONLY: five (off, size) pairs after cmd/cmdsize.
LC_DYLD_INFO_CMDS = {0x22, 0x80000022}


class Unsupported(Exception):
    """The file is something this script should not touch."""


def align_file(path: str, verbose: bool = False) -> bool:
    """Repair `path` in place. Returns True if it was changed."""
    if not needs_padding(path):
        if verbose:
            print(f"{path}: string pool already aligned")
        return False

    # The ad-hoc signature the link produced covers the bytes about to move, and
    # `codesign` will not replace a signature in a file it reads as
    # inconsistent — so the signature comes off first, with Apple's own tool,
    # and goes back on afterwards. Between the two the file is unsigned, which
    # is why it is repaired in the build tree and not in place after install.
    subprocess.run(
        ["codesign", "--remove-signature", path], check=True, capture_output=True
    )

    with open(path, "rb") as f:
        data = bytearray(f.read())

    ncmds = struct.unpack_from("<I", data, 16)[0]

    # Collect every field that holds a file offset into __LINKEDIT, so they can
    # be moved together. Each entry is (offset_of_field_in_file, value).
    offsets: list[tuple[int, int]] = []
    stroff_field = None
    stroff = None
    linkedit_cmd = None  # (offset of the segment command, fileoff, filesize)

    pos = 32
    for _ in range(ncmds):
        cmd, cmdsize = struct.unpack_from("<II", data, pos)
        if cmdsize == 0:
            raise Unsupported("a load command has size 0")
        if cmd == LC_SEGMENT_64:
            name = bytes(data[pos + 8 : pos + 24]).rstrip(b"\0")
            if name == b"__LINKEDIT":
                # segment_command_64: cmd 0, cmdsize 4, segname 8, vmaddr 24,
                # vmsize 32, fileoff 40, filesize 48, maxprot 56, initprot 60.
                fileoff, filesize = struct.unpack_from("<QQ", data, pos + 40)
                linkedit_cmd = (pos, fileoff, filesize)
        elif cmd == LC_SYMTAB:
            symoff, _nsyms, stroff, _strsize = struct.unpack_from("<IIII", data, pos + 8)
            offsets.append((pos + 8, symoff))
            stroff_field = pos + 16
        elif cmd == LC_DYSYMTAB:
            # Only the four offsets that are non-zero in practice; a zero
            # offset means "no such table" and must stay zero.
            for i, field in enumerate(("tocoff", "modtaboff", "extrefsymoff", "indirectsymoff",
                                       "extreloff", "locreloff")):
                at = pos + 8 + 32 + i * 8
                value = struct.unpack_from("<I", data, at)[0]
                if value:
                    offsets.append((at, value))
        elif cmd in LINKEDIT_DATA_CMDS:
            value = struct.unpack_from("<I", data, pos + 8)[0]
            if value:
                offsets.append((pos + 8, value))
        elif cmd in LC_DYLD_INFO_CMDS:
            for i in range(5):
                at = pos + 8 + i * 8
                value = struct.unpack_from("<I", data, at)[0]
                if value:
                    offsets.append((at, value))
        pos += cmdsize

    if stroff is None or stroff_field is None:
        raise Unsupported("no LC_SYMTAB")
    if linkedit_cmd is None:
        raise Unsupported("no __LINKEDIT segment")

    pad = (8 - stroff % 8) % 8
    if pad == 0:
        # Removing the signature does not move the string table, so this only
        # happens if the file changed underneath us.
        raise Unsupported("string pool became aligned while being repaired")

    # Insert the padding at the string table and move everything at or after it.
    data[stroff:stroff] = b"\0" * pad
    struct.pack_into("<I", data, stroff_field, stroff + pad)
    for at, value in offsets:
        if value >= stroff:
            struct.pack_into("<I", data, at, value + pad)

    # __LINKEDIT grew. Its vmsize is page-rounded and must still cover it.
    seg_pos, _fileoff, filesize = linkedit_cmd
    filesize += pad
    struct.pack_into("<Q", data, seg_pos + 48, filesize)
    vmsize = struct.unpack_from("<Q", data, seg_pos + 32)[0]
    page = 0x4000
    needed = (filesize + page - 1) // page * page
    if vmsize < needed:
        struct.pack_into("<Q", data, seg_pos + 32, needed)

    # Written via a temporary file and moved into place, so an interrupted run
    # cannot leave a half-written library behind.
    tmp = path + ".aligning"
    with open(tmp, "wb") as f:
        f.write(data)
    shutil.copymode(path, tmp)
    shutil.move(tmp, path)

    # Sign it again, ad-hoc, the way the linker did.
    subprocess.run(
        ["codesign", "--force", "--sign", "-", path], check=True, capture_output=True
    )

    # Check the result rather than trust the arithmetic: this rewrites a
    # Mach-O by hand, and a file that is wrong in some other way would
    # otherwise reach a user as a cryptic dyld error. There is no unit test
    # behind the surgery — a misaligned Mach-O cannot be checked in as
    # fixture data — so this is where it is verified.
    if needs_padding(path):
        raise Unsupported("padding did not align the string pool")
    subprocess.run(["codesign", "--verify", path], check=True, capture_output=True)
    print(f"{path}: padded the string pool by {pad} bytes ({stroff:#x} -> {stroff + pad:#x})")
    return True


def needs_padding(path: str) -> bool:
    """Whether `path` is a 64-bit Mach-O whose string pool is misaligned.
    Raises [`Unsupported`] for a file this script must not touch."""
    with open(path, "rb") as f:
        head = f.read(4)
        if len(head) < 4:
            raise Unsupported("too short to be a Mach-O")
        if struct.unpack(">I", head)[0] in (FAT_MAGIC, FAT_CIGAM):
            # A universal binary holds several Mach-Os at aligned offsets; each
            # would need repairing and the fat header rewritten. Nothing here
            # produces one, so refuse rather than corrupt it.
            raise Unsupported("universal (fat) binaries are not supported")
        if struct.unpack("<I", head)[0] != MH_MAGIC_64:
            if struct.unpack("<I", head)[0] == MH_CIGAM_64:
                raise Unsupported("byte-swapped Mach-O is not supported")
            raise Unsupported("not a 64-bit Mach-O")
        f.seek(0)
        data = f.read()

    ncmds = struct.unpack_from("<I", data, 16)[0]
    pos = 32
    for _ in range(ncmds):
        cmd, cmdsize = struct.unpack_from("<II", data, pos)
        if cmdsize == 0:
            raise Unsupported("a load command has size 0")
        if cmd == LC_SYMTAB:
            stroff = struct.unpack_from("<I", data, pos + 16)[0]
            return stroff % 8 != 0
        pos += cmdsize
    raise Unsupported("no LC_SYMTAB")


def main(argv: list[str]) -> int:
    verbose = "-v" in argv
    check_only = "--check" in argv
    paths = [a for a in argv[1:] if not a.startswith("-")]
    if not paths:
        print(__doc__, file=sys.stderr)
        return 2
    status = 0
    for path in paths:
        try:
            if check_only:
                if needs_padding(path):
                    print(
                        f"{path}: its symbol string pool is misaligned, so macOS will "
                        f"refuse to load it — run {sys.argv[0]} on it",
                        file=sys.stderr,
                    )
                    status = 1
                elif verbose:
                    print(f"{path}: string pool aligned")
            else:
                align_file(path, verbose)
        except Unsupported as e:
            # Not something to repair — an ELF binary on Linux, a script, a
            # universal binary. Only worth a word when asked for detail.
            if verbose:
                print(f"{path}: skipped — {e}", file=sys.stderr)
        except subprocess.CalledProcessError as e:
            tool = e.cmd[0] if e.cmd else "a tool"
            err = (e.stderr or b"").decode(errors="replace").strip()
            print(f"{path}: {tool} failed: {err}", file=sys.stderr)
            status = 1
        except OSError as e:
            print(f"{path}: {e}", file=sys.stderr)
            status = 1
    return status


if __name__ == "__main__":
    sys.exit(main(sys.argv))
