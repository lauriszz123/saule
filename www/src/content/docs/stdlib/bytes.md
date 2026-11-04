---
title: "Bytes"
description: "A bytes is a buffer of arbitrary bytes with no encoding promised, measured and indexed in bytes. It is a primitive type alongside string, and the two…"
sidebar:
  order: 3
---

<!-- Generated from DOCS.md by `npm run sync-docs`. Edit that file, not this one. -->

A `bytes` is a buffer of arbitrary bytes with no encoding promised, measured
and indexed in bytes. It is a primitive type alongside `string`, and the two
are **not interchangeable**: a `string` is text, always valid UTF-8, measured
in codepoints.

Reach for `bytes` whenever the data is not text — an image, a digest, a
compressed blob, a wire protocol. Reach for `string` for everything else.

```saule
local raw: bytes = Bytes.of(0x89, 0x50, 0x4e, 0x47)

println(#raw)                            -- 4, a byte count
println(Bytes.toHex(raw))                -- 89504e47
println(Bytes.toString(raw) == nil)      -- true, not valid UTF-8
```

Lua makes one type do both jobs, which is why `#s` there is a byte count and
`s:sub(1, 1)` can cut a character in half. Keeping them apart lets the
typechecker enforce the difference, so a buffer can never be mistaken for
text.

Two things `bytes` deliberately does **not** do. `..` does not accept it —
concatenation builds a string, and a buffer that is not text has no string
form, so use `Bytes.concat`. And it cannot be a table key: key by
`Bytes.toHex(b)` instead. Equality is by contents, and a `bytes` is never
equal to a `string`.

Indices are **1-based**, as everywhere else in Saule.

### Building

<div class="api-list">
<details class="api-entry" id="bytes-of">
<summary><code class="api-sig"><span class="sig-fn">Bytes.of</span><span class="sig-punct">(</span><span class="sig-punct">...</span><span class="sig-type">byte</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">bytes</span></code></summary>


Builds a buffer from byte values. The arguments are typed
[`byte`](../README.md#the-byte-type), so a literal outside `0..=255` is a
**compile** error rather than a runtime surprise, and a computed value has to
narrow with `as byte` first.

Note the difference from `String.char`: `Bytes.of(0x80)` is **one** byte,
where `String.char(0x80)` is the two-byte UTF-8 encoding of U+0080.

```saule
local png: bytes = Bytes.of(0x89, 0x50, 0x4e, 0x47)
println(Bytes.toHex(png))                -- 89504e47
println(#png)                            -- 4

-- One byte, not a character.
println(#Bytes.of(0x80))                 -- 1
println(#Bytes.fromString(String.char(0x80)))   -- 2

-- A computed value narrows explicitly.
local n: integer = 200
local b: byte? = n as byte
println(Bytes.toHex(Bytes.of(b!)))       -- c8
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="bytes-empty">
<summary><code class="api-sig"><span class="sig-fn">Bytes.empty</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">bytes</span></code></summary>


The empty buffer. Useful as a starting value for an accumulator, and as the
fallback when a read returns nothing.

```saule
println(#Bytes.empty())                  -- 0
println(Bytes.toHex(Bytes.empty()))      -- (empty)

-- Accumulating.
local acc: bytes = Bytes.empty()
for n in {1, 2, 3} do
    acc = Bytes.concat(acc, Bytes.of((n as byte) ?? 0))
end
println(Bytes.toHex(acc))                -- 010203
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="bytes-concat">
<summary><code class="api-sig"><span class="sig-fn">Bytes.concat</span><span class="sig-punct">(</span><span class="sig-punct">...</span><span class="sig-type">bytes</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">bytes</span></code></summary>


Joins buffers in order. This is what `..` would do if it accepted `bytes` —
it does not, because concatenation builds a string.

```saule
local a: bytes = Bytes.of(0x01, 0x02)
local b: bytes = Bytes.of(0x03)

println(Bytes.toHex(Bytes.concat(a, b)))            -- 010203
println(Bytes.toHex(Bytes.concat(a, b, a)))         -- 0102030102
println(#Bytes.concat(Bytes.empty(), a))            -- 2

-- Building a framed message: one length byte, then the body. The length is
-- parenthesised because `as` binds tighter than `#`.
local body: bytes = Bytes.fromString("hi")
local size: byte = ((#body) as byte) ?? 0
println(Bytes.toHex(Bytes.concat(Bytes.of(size), body)))   -- 026869
```


</details>
</div>

### Converting to and from `string`

<div class="api-list">
<details class="api-entry" id="bytes-fromstring">
<summary><code class="api-sig"><span class="sig-fn">Bytes.fromString</span><span class="sig-punct">(</span><span class="sig-param">s</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">bytes</span></code></summary>


The UTF-8 bytes of `s`. Always succeeds, because every Saule string is
already valid UTF-8.

```saule
println(Bytes.toHex(Bytes.fromString("Hi")))   -- 4869
println(#Bytes.fromString("saulė"))            -- 6 bytes
println(String.len("saulė"))                   -- 5 characters

-- The byte count of a string, which `String.len` does not give you.
local text: string = "héllo"
println(#Bytes.fromString(text))               -- 6
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="bytes-tostring">
<summary><code class="api-sig"><span class="sig-fn">Bytes.toString</span><span class="sig-punct">(</span><span class="sig-param">b</span><span class="sig-punct">:</span> <span class="sig-type">bytes</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">string</span><span class="sig-punct">?</span></code></summary>


The buffer as text, or `nil` when it is not valid UTF-8.

Nullable on purpose: there is no total function from bytes to text, and
silently substituting U+FFFD would hide the problem at exactly the point it
matters.

```saule
println(Bytes.toString(Bytes.of(0x68, 0x69)) ?? "?")   -- hi
println(Bytes.toString(Bytes.of(0xff)) == nil)         -- true

-- The shape this is written for: decode, or fall back.
local payload: bytes = Bytes.fromString("ok")
local text: string = Bytes.toString(payload) ?? "<binary>"
println(text)                                          -- ok
```


</details>
</div>

### Inspecting

<div class="api-list">
<details class="api-entry" id="bytes-len">
<summary><code class="api-sig"><span class="sig-fn">Bytes.len</span><span class="sig-punct">(</span><span class="sig-param">b</span><span class="sig-punct">:</span> <span class="sig-type">bytes</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">integer</span></code></summary>


The byte count. `#b` is the same thing, as a value you can pass around.

```saule
local raw: bytes = Bytes.of(1, 2, 3)
println(Bytes.len(raw))        -- 3
println(#raw)                  -- 3, the same number
println(Bytes.len(Bytes.empty()))   -- 0
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="bytes-byte">
<summary><code class="api-sig"><span class="sig-fn">Bytes.byte</span><span class="sig-punct">(</span><span class="sig-param">b</span><span class="sig-punct">:</span> <span class="sig-type">bytes</span><span class="sig-punct">,</span> <span class="sig-param">i</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">byte</span><span class="sig-punct">?</span></code></summary>


The byte at `i`, or `nil` when `i` is outside the buffer. The result is a
[`byte`](../README.md#the-byte-type), which is in `0..=255` by construction
and widens to `integer` for free.

```saule
local raw: bytes = Bytes.of(0x7f, 0x80, 0xff)

println(Bytes.byte(raw, 1) ?? 0)     -- 127
println(Bytes.byte(raw, 2) ?? 0)     -- 128
println(Bytes.byte(raw, 9) == nil)   -- true

-- Summing a buffer, stopping cleanly at the end.
local total: integer = 0
local i: integer = 1
while Bytes.byte(raw, i) != nil do
    total = total + (Bytes.byte(raw, i) ?? 0)
    i = i + 1
end
println(total)                       -- 510
```


</details>
</div>

### Slicing

<div class="api-list">
<details class="api-entry" id="bytes-sub">
<summary><code class="api-sig"><span class="sig-fn">Bytes.sub</span><span class="sig-punct">(</span><span class="sig-param">b</span><span class="sig-punct">:</span> <span class="sig-type">bytes</span><span class="sig-punct">,</span> <span class="sig-param">from</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">,</span> <span class="sig-param">to</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">?</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">bytes</span></code></summary>


The slice from `from` to `to`, both inclusive and 1-based. `to` defaults to
the end, negatives count from the end, and both ends clamp — so an
out-of-range slice is empty rather than an error.

```saule
local raw: bytes = Bytes.of(1, 2, 3, 4, 5)

println(Bytes.toHex(Bytes.sub(raw, 2, 4)))    -- 020304
println(Bytes.toHex(Bytes.sub(raw, 3)))       -- 030405
println(Bytes.toHex(Bytes.sub(raw, -2)))      -- 0405
println(#Bytes.sub(raw, 99, 120))             -- 0, clamped

-- Splitting a header off a payload.
local frame: bytes = Bytes.of(0x02, 0xaa, 0xbb)
println(Bytes.toHex(Bytes.sub(frame, 1, 1)))  -- 02, the length byte
println(Bytes.toHex(Bytes.sub(frame, 2)))     -- aabb, the body
```


</details>
</div>

### Searching

<div class="api-list">
<details class="api-entry" id="bytes-indexof">
<summary><code class="api-sig"><span class="sig-fn">Bytes.indexOf</span><span class="sig-punct">(</span><span class="sig-param">b</span><span class="sig-punct">:</span> <span class="sig-type">bytes</span><span class="sig-punct">,</span> <span class="sig-param">needle</span><span class="sig-punct">:</span> <span class="sig-type">bytes</span><span class="sig-punct">,</span> <span class="sig-param">from</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">?</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">integer</span><span class="sig-punct">?</span></code></summary>


The 1-based position where `needle` first appears, or `nil` when it does not.
`from` says where to start. An empty needle matches at `from`.

```saule
local raw: bytes = Bytes.of(0x00, 0x7f, 0x80, 0x7f)

println(Bytes.indexOf(raw, Bytes.of(0x80)) ?? 0)       -- 3
println(Bytes.indexOf(raw, Bytes.of(0x7f)) ?? 0)       -- 2, the first one
println(Bytes.indexOf(raw, Bytes.of(0x7f), 3) ?? 0)    -- 4, searching on
println(Bytes.indexOf(raw, Bytes.of(0xee)) == nil)     -- true

-- Finding a delimiter in a buffer.
local packet: bytes = Bytes.fromString("key=value")
local at: integer? = Bytes.indexOf(packet, Bytes.fromString("="))
if at != nil then
    println(Bytes.toString(Bytes.sub(packet, 1, at! - 1)) ?? "")   -- key
end
```


</details>
</div>

### Hexadecimal

<div class="api-list">
<details class="api-entry" id="bytes-tohex">
<summary><code class="api-sig"><span class="sig-fn">Bytes.toHex</span><span class="sig-punct">(</span><span class="sig-param">b</span><span class="sig-punct">:</span> <span class="sig-type">bytes</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">string</span></code></summary>


Lowercase hexadecimal, two characters per byte.

This is how a buffer gets looked at — `print(b)` shows only `<bytes len=N>` —
and how one becomes a table key, since `bytes` is not keyable itself.

```saule
println(Bytes.toHex(Bytes.of(0, 255)))        -- 00ff
println(Bytes.toHex(Bytes.of(0xde, 0xad)))    -- dead
println(Bytes.toHex(Bytes.empty()))           -- (empty)

-- Keying a table by a buffer's contents.
local seen: table<string, boolean> = {}
local digest: bytes = Bytes.of(0x01, 0x02)
seen[Bytes.toHex(digest)] = true
println(seen["0102"] ?? false)                -- true
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="bytes-fromhex">
<summary><code class="api-sig"><span class="sig-fn">Bytes.fromHex</span><span class="sig-punct">(</span><span class="sig-param">s</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">bytes</span><span class="sig-punct">?</span></code></summary>


Parses hexadecimal back into a buffer. `nil` for an odd number of digits or
any non-hex character, rather than a buffer that is quietly half right.

```saule
println(Bytes.toHex(Bytes.fromHex("00ff")!))   -- 00ff
println(Bytes.fromHex("0f0") == nil)           -- true, odd length
println(Bytes.fromHex("zz") == nil)            -- true, not hex

-- Round trip.
local raw: bytes = Bytes.of(0xca, 0xfe)
println(Bytes.toHex(Bytes.fromHex(Bytes.toHex(raw))!) == Bytes.toHex(raw))   -- true
```


</details>
</div>

### Byte arrays

<div class="api-list">
<details class="api-entry" id="bytes-toarray">
<summary><code class="api-sig"><span class="sig-fn">Bytes.toArray</span><span class="sig-punct">(</span><span class="sig-param">b</span><span class="sig-punct">:</span> <span class="sig-type">bytes</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">byte</span><span class="sig-punct">[</span><span class="sig-punct">]</span></code></summary>


The buffer as one element per byte. The expensive direction — every byte
becomes a boxed table element — and explicit about it.

```saule
local raw: bytes = Bytes.of(10, 20, 30)
local list: byte[] = Bytes.toArray(raw)

println(#list)                    -- 3
println(list[2] ?? 0)             -- 20

-- Which makes the whole sequence API available.
println(Iter.reduce(list, 0, (acc, n) => acc + n))   -- 60
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="bytes-fromarray">
<summary><code class="api-sig"><span class="sig-fn">Bytes.fromArray</span><span class="sig-punct">(</span><span class="sig-param">a</span><span class="sig-punct">:</span> <span class="sig-type">byte</span><span class="sig-punct">[</span><span class="sig-punct">]</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">bytes</span><span class="sig-punct">?</span></code></summary>


Packs elements back into a buffer. `nil` when an element is out of range —
which the typechecker prevents for a `byte[]`, but not for a table that
arrived through `any`.

```saule
println(Bytes.toHex(Bytes.fromArray({1, 2, 3})!))    -- 010203
println(#Bytes.fromArray({})!)                       -- 0

-- Round trip through the elementwise form.
local raw: bytes = Bytes.of(0xaa, 0xbb)
println(Bytes.toHex(Bytes.fromArray(Bytes.toArray(raw))!))   -- aabb
```


</details>
</div>

### `bytes` and `byte[]`

A `bytes` is the packed representation; a `byte[]` (that is,
`table<integer, byte>`) is the elementwise one. `Bytes.toArray` and
`Bytes.fromArray` convert between them, and they are two explicit functions
rather than an implicit conversion because the costs are not comparable: a
`bytes` is one byte per byte, while a table holds a boxed value per element.

Reach for `bytes` to hold or move bytes — a file, a socket frame, a digest,
anything crossing into a native package. Reach for `byte[]` only to work
through bytes one at a time, where a loop over elements is clearer than
repeated `Bytes.byte` calls.
