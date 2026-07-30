---
title: "Io / File"
description: "File handles are reference-counted File values. Io is the static entry point and also holds the standard streams. Mode and seek-whence are real Saule…"
sidebar:
  order: 5
---

<!-- Generated from DOCS.md by `npm run sync-docs`. Edit that file, not this one. -->

File handles are reference-counted `File` values. `Io` is the static entry
point and also holds the standard streams. Mode and seek-whence are real
Saule enums (`IoMode`, `IoSeek`) so the typechecker can catch typos.

### Streams

| Name | Type | Description |
| --- | --- | --- |
| `Io.stdin` | `File` | Read-only standard input. |
| `Io.stdout` | `File` | Write-only standard output. |
| `Io.stderr` | `File` | Write-only standard error. |

### Static functions

| Signature | Description |
| --- | --- |
| `Io.open(path: string, mode: IoMode) -> File?` | Open a file; `nil` on error. |
| `Io.lines(path: string?) -> fn(): string?` | Line-iterator step closure. With no path, reads stdin. |
| `Io.read(...string) -> string?` | Read formats from stdin: `"l"`/`"L"` (line), `"a"` (all), `"n"` (number), or a numeric byte count. |
| `Io.write(...string) -> nil` | Write to stdout. |

### `IoMode` variants

| Variant | Underlying mode |
| --- | --- |
| `Read` | `r` |
| `Write` | `w` |
| `Append` | `a` |
| `ReadWrite` | `r+` |
| `WriteRead` | `w+` |
| `AppendRead` | `a+` |
| `ReadBinary` | `rb` |
| `WriteBinary` | `wb` |
| `AppendBinary` | `ab` |

### `IoSeek` variants

`Set` (`set`), `Cur` (`cur`), `End` (`end`).

### `File` methods (dot-style)

| Signature | Description |
| --- | --- |
| `f.read(...string) -> string?` | Same formats as `Io.read`. |
| `f.write(...string) -> nil` | Append bytes. |
| `f.lines() -> fn(): string?` | Per-line step closure. |
| `f.seek(whence: IoSeek?, offset: integer?) -> integer` | Defaults `(Cur, 0)`. Returns new position. |
| `f.flush() -> nil` | Force buffered writes to disk. |
| `f.close() -> nil` | Release the underlying handle. |

```saule
local f: File = Io.open("/tmp/notes.txt", IoMode.Write)!
f.write("hello\n")
f.close()

for line in Io.lines("/tmp/notes.txt") do
    println(line)
end
```

---
