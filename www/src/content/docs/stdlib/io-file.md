---
title: "Io / File"
description: "Io is the static entry point and holds the standard streams; a File is a reference-counted handle to an open file. Both the mode a file is opened in…"
sidebar:
  order: 7
---

<!-- Generated from DOCS.md by `npm run sync-docs`. Edit that file, not this one. -->

`Io` is the static entry point and holds the standard streams; a `File` is a
reference-counted handle to an open file. Both the mode a file is opened in
and the whence of a seek are real Saule enums — `IoMode` and `IoSeek` — so a
typo is a compile error rather than a silent failure at run time.

The examples below write under `Os.tmpname()` so they run anywhere without
leaving anything behind.

```saule
local path: string = Os.tmpname()

local out: File = Io.open(path, IoMode.Write)!
out.write("first\n")
out.write("second\n")
out.close()

for line in Io.lines(path) do
    println(line)              -- first, then second
end

Os.remove(path)
```

### Streams

`Io.stdin`, `Io.stdout` and `Io.stderr` are `File` values that are already
open. They need no `Io.open` and should not be closed.

| Name | Type | What it holds |
| --- | --- | --- |
| `Io.stdin` | `File` | This process's standard input, already open for reading |
| `Io.stdout` | `File` | This process's standard output. `println` writes here |
| `Io.stderr` | `File` | This process's standard error, for diagnostics that must not be piped away with the output |

```saule
Io.stdout.write("to stdout\n")
Io.stderr.write("to stderr\n")

-- `println` is `Io.stdout.write` with a newline and `tostring` applied, so
-- the two interleave exactly as written.
Io.stdout.write("a")
println("b")                   -- ab
```

### `IoMode` variants

| Variant | Mode | What it does |
| --- | --- | --- |
| `Read` | `r` | Read only; `Io.open` answers `nil` when the file does not exist |
| `Write` | `w` | Write only, **truncating** an existing file to empty |
| `Append` | `a` | Write only, every write going to the end |
| `ReadWrite` | `r+` | Read and write, file must exist, not truncated |
| `WriteRead` | `w+` | Read and write, truncating |
| `AppendRead` | `a+` | Read anywhere, writes always at the end |
| `ReadBinary` | `rb` | As `Read`, in binary mode |
| `WriteBinary` | `wb` | As `Write`, in binary mode |
| `AppendBinary` | `ab` | As `Append`, in binary mode |

The binary modes matter on Windows, where a text-mode handle translates
`\n` to `\r\n` on the way out and back on the way in. For `bytes` that is
corruption, so anything that is not text wants a binary mode.

### `IoSeek` variants

`Set` counts from the start of the file, `Cur` from the current position and
`End` from the end (where a negative offset moves backwards).

### Static functions

<div class="api-list">
<details class="api-entry" id="io-open">
<summary><code class="api-sig"><span class="sig-fn">Io.open</span><span class="sig-punct">(</span><span class="sig-param">path</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">,</span> <span class="sig-param">mode</span><span class="sig-punct">:</span> <span class="sig-type">IoMode</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">File</span><span class="sig-punct">?</span></code></summary>


Opens `path` and answers a handle, or `nil` when it cannot — the file is
missing, the directory is not writable, the path is a directory.

Nullable rather than throwing, because a file that is not there is an
ordinary condition rather than an exceptional one.

```saule
local path: string = Os.tmpname()

-- Writing creates the file.
local out: File = Io.open(path, IoMode.Write)!
out.write("hello")
out.close()

-- Reading finds it.
local input: File = Io.open(path, IoMode.Read)!
println(input.read("a") ?? "")        -- hello
input.close()

-- A file that is not there is `nil`, not an error.
println(Io.open("/no/such/file", IoMode.Read) == nil)   -- true

Os.remove(path)
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="io-lines">
<summary><code class="api-sig"><span class="sig-fn">Io.lines</span><span class="sig-punct">(</span><span class="sig-param">path</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">?</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-kw">fn</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">string</span><span class="sig-punct">?</span></code></summary>


A step closure over the lines of `path`, written for `for`. With no argument
it reads standard input instead.

The line separator is not included in what comes back, and the closure
answers `nil` at end of file, which is what ends the loop.

```saule
local path: string = Os.tmpname()
local out: File = Io.open(path, IoMode.Write)!
out.write("alpha\nbeta\ngamma\n")
out.close()

for line in Io.lines(path) do
    println("[" .. line .. "]")    -- [alpha], [beta], [gamma]
end

-- It is an ordinary step closure, so `Iter.collect` takes it too.
local all: table<string> = Iter.collect(Io.lines(path))
println(#all)                      -- 3

Os.remove(path)
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="io-read">
<summary><code class="api-sig"><span class="sig-fn">Io.read</span><span class="sig-punct">(</span><span class="sig-punct">...</span><span class="sig-type">string</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">string</span><span class="sig-punct">?</span></code></summary>


Reads from standard input in the given formats:

| Format | Reads |
| --- | --- |
| `"l"` | One line, without its newline |
| `"L"` | One line, keeping its newline |
| `"a"` | Everything remaining, as one string |
| `"n"` | One line, parsed as an `integer`; `nil` when it does not parse |

A leading `*` is accepted on any of them (`"*l"`), for familiarity with Lua.
There is **no** numeric byte-count format — for a fixed number of bytes, use
[`f.readBytes(count)`](/saule/stdlib/io-file/#f-readbytes).

`nil` at end of input, which is how a read loop knows to stop.

```saule
-- Reading a line from stdin, with a fallback when there is none.
local answer: string = Io.read("l") ?? "<no input>"
println(answer)
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="io-write">
<summary><code class="api-sig"><span class="sig-fn">Io.write</span><span class="sig-punct">(</span><span class="sig-punct">...</span><span class="sig-type">string</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-kw">nil</span></code></summary>


Writes its arguments to standard output with nothing between them and no
trailing newline. `Io.stdout.write(...)` is the same thing.

```saule
Io.write("no", " ", "newline")
Io.write("\n")
println("done")                -- no newline / done
```


</details>
</div>

### `File` methods

These are called with a dot, like every other method in Saule:
`handle.read("a")`, never `handle:read("a")`.

<div class="api-list">
<details class="api-entry" id="f-read">
<summary><code class="api-sig"><span class="sig-fn">f.read</span><span class="sig-punct">(</span><span class="sig-punct">...</span><span class="sig-type">string</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">string</span><span class="sig-punct">?</span></code></summary>


Reads from the handle in the same formats [`Io.read`](/saule/stdlib/io-file/#io-read) takes —
`"l"`, `"L"`, `"a"` and `"n"`. The result is a `string`, so this is for
**text**: a file that is not valid UTF-8 needs `readBytes`, and so does a
fixed number of bytes.

```saule
local path: string = Os.tmpname()
local out: File = Io.open(path, IoMode.Write)!
out.write("line one\nline two\n")
out.close()

local f: File = Io.open(path, IoMode.Read)!
println(f.read("l") ?? "")       -- line one, newline stripped
println(f.read("L") ?? "")       -- line two, newline kept, so this prints blank after
println(f.read("l") == nil)      -- true, at end of file
f.close()

Os.remove(path)
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="f-readbytes">
<summary><code class="api-sig"><span class="sig-fn">f.readBytes</span><span class="sig-punct">(</span><span class="sig-param">count</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">?</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">bytes</span><span class="sig-punct">?</span></code></summary>


Reads raw [`bytes`](/saule/stdlib/bytes/), assuming no encoding. With no argument it reads
to end of file; with a `count` it reads up to that many and answers `nil`
only at end of file — so a read loop terminates.

```saule
local path: string = Os.tmpname()
local out: File = Io.open(path, IoMode.WriteBinary)!
out.writeBytes(Bytes.of(0x00, 0x7f, 0x80, 0xff))
out.close()

local f: File = Io.open(path, IoMode.ReadBinary)!
local raw: bytes = f.readBytes() ?? Bytes.empty()
println(Bytes.toHex(raw))        -- 007f80ff
println(Bytes.toString(raw) == nil)   -- true, not text
f.close()

-- Reading in fixed-size chunks until the end.
local g: File = Io.open(path, IoMode.ReadBinary)!
local total: integer = 0
while true do
    local chunk: bytes? = g.readBytes(2)
    if chunk == nil then
        break
    end
    total = total + #chunk!
end
println(total)                   -- 4
g.close()

Os.remove(path)
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="f-write">
<summary><code class="api-sig"><span class="sig-fn">f.write</span><span class="sig-punct">(</span><span class="sig-punct">...</span><span class="sig-type">string</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-kw">nil</span></code></summary>


Appends text at the handle's current position. A `bytes` passed here would be
rendered as `<bytes len=N>` — use `writeBytes`.

```saule
local path: string = Os.tmpname()
local f: File = Io.open(path, IoMode.Write)!
f.write("a", "b", "c")           -- nothing between the arguments
f.write("\n")
f.close()

println(Io.open(path, IoMode.Read)!.read("a") ?? "")   -- abc
Os.remove(path)
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="f-writebytes">
<summary><code class="api-sig"><span class="sig-fn">f.writeBytes</span><span class="sig-punct">(</span><span class="sig-param">data</span><span class="sig-punct">:</span> <span class="sig-type">bytes</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-kw">nil</span></code></summary>


Appends the bytes themselves, with no encoding applied. Writing non-UTF-8 to
stdout or stderr is an error rather than mojibake.

```saule
local path: string = Os.tmpname()
local f: File = Io.open(path, IoMode.WriteBinary)!
f.writeBytes(Bytes.fromString("PNG"))
f.writeBytes(Bytes.of(0x0d, 0x0a))
f.close()

local back: File = Io.open(path, IoMode.ReadBinary)!
println(Bytes.toHex(back.readBytes() ?? Bytes.empty()))   -- 504e470d0a
back.close()

Os.remove(path)
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="f-lines">
<summary><code class="api-sig"><span class="sig-fn">f.lines</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-kw">fn</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">string</span><span class="sig-punct">?</span></code></summary>


A step closure over the handle's remaining lines. The same as `Io.lines`, for
a file you have already opened — which is what you want when the handle was
opened in a particular mode, or when some of it has already been read.

```saule
local path: string = Os.tmpname()
local out: File = Io.open(path, IoMode.Write)!
out.write("one\ntwo\nthree\n")
out.close()

local f: File = Io.open(path, IoMode.Read)!
println(f.read("l") ?? "")       -- one, read directly

for line in f.lines() do
    println(line)                -- two, then three: it carries on
end
f.close()

Os.remove(path)
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="f-seek">
<summary><code class="api-sig"><span class="sig-fn">f.seek</span><span class="sig-punct">(</span><span class="sig-param">whence</span><span class="sig-punct">:</span> <span class="sig-type">IoSeek</span><span class="sig-punct">?</span><span class="sig-punct">,</span> <span class="sig-param">offset</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">?</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">integer</span><span class="sig-punct">?</span></code></summary>


Moves the read/write position and answers the new position, counted in bytes
from the start. Defaults to `(IoSeek.Cur, 0)`, which moves nothing and so
answers where you are.

`nil` when the handle cannot seek — a pipe, or a closed file.

```saule
local path: string = Os.tmpname()
local out: File = Io.open(path, IoMode.Write)!
out.write("0123456789")
out.close()

local f: File = Io.open(path, IoMode.ReadBinary)!
println(f.seek() ?? -1)                       -- 0, where we are
println(Bytes.toString(f.readBytes(2) ?? Bytes.empty()) ?? "")   -- 01
println(f.seek() ?? -1)                       -- 2, two bytes in

f.seek(IoSeek.Set, 5)
println(Bytes.toString(f.readBytes(2) ?? Bytes.empty()) ?? "")   -- 56

-- From the end, with a negative offset.
f.seek(IoSeek.End, -3)
println(f.read("a") ?? "")                    -- 789

-- The size of a file, without reading it.
println(f.seek(IoSeek.End, 0) ?? -1)          -- 10
f.close()

Os.remove(path)
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="f-flush">
<summary><code class="api-sig"><span class="sig-fn">f.flush</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-kw">nil</span></code></summary>


Forces buffered writes out to the file. Writes are buffered, so another
reader — a different handle, another process — does not see them until the
buffer is flushed or the handle is closed.

```saule
local path: string = Os.tmpname()
local f: File = Io.open(path, IoMode.Write)!
f.write("buffered")
f.flush()

-- Now a second handle can see it, without closing the first.
println(Io.open(path, IoMode.Read)!.read("a") ?? "")   -- buffered

f.close()
Os.remove(path)
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="f-close">
<summary><code class="api-sig"><span class="sig-fn">f.close</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-kw">nil</span></code></summary>


Releases the handle, flushing whatever is buffered. Using a handle after
closing it is an error, so close last.

```saule
local path: string = Os.tmpname()
local f: File = Io.open(path, IoMode.Write)!
f.write("done")
f.close()

-- The data is on disk, without an explicit flush.
println(Io.open(path, IoMode.Read)!.read("a") ?? "")   -- done
Os.remove(path)
```

</details>
</div>

