---
title: "Bytes"
description: "Byte-string utilities. bytes is a primitive type alongside string, and the two are not interchangeable: a string is text, always valid UTF-8, with…"
---

<!-- Generated from DOCS.md by `npm run sync-docs`. Edit that file, not this one. -->

Byte-string utilities. `bytes` is a primitive type alongside `string`, and the
two are **not interchangeable**: a `string` is text, always valid UTF-8, with
`String.len` and `String.sub` measured in codepoints; a `bytes` is a buffer of
arbitrary bytes with no encoding promised, measured and indexed in bytes.

Reach for `bytes` whenever the data is not text — an image, a digest, a
compressed blob, a wire protocol. Reach for `string` for everything else.

Lua makes one type do both jobs, which is why `#s` there is a byte count and
`s:sub(1, 1)` can cut a character in half. Saule keeps them apart and lets the
typechecker enforce it, so a buffer can never be mistaken for text:

```saule
local b: bytes = Bytes.of(0x89, 0x50, 0x4e, 0x47)
local s: string = b              -- error: cannot assign `bytes` to `string`
println(String.len(b))           -- error: expects `string`, got `bytes`
```

Indices are **1-based**, as everywhere else in Saule, and `Bytes.sub` takes
negatives the way `String.sub` does.

| Signature | Description |
| --- | --- |
| `Bytes.of(...integer) -> bytes` | Build a buffer from byte values. Each must be `0..=255`; anything else is an error rather than a truncation. `Bytes.of(0x80)` is **one** byte, where `String.char(0x80)` is the two-byte UTF-8 encoding of U+0080. |
| `Bytes.empty() -> bytes` | The empty buffer. |
| `Bytes.fromString(s: string) -> bytes` | The string's UTF-8 bytes. Always succeeds — every Saule string is already valid UTF-8. |
| `Bytes.toString(b: bytes) -> string?` | The buffer as text, or `nil` when it is not valid UTF-8. Nullable on purpose: there is no total function from bytes to text, and silently substituting U+FFFD would hide the problem. |
| `Bytes.len(b: bytes) -> integer` | Byte count. `#b` is the same thing. |
| `Bytes.byte(b: bytes, i: integer) -> integer?` | The byte at `i`, or `nil` out of range — so a scan can walk off the end and stop. |
| `Bytes.sub(b: bytes, from: integer, to: integer?) -> bytes` | Slice; `to` defaults to the end, negatives count from the end, both ends clamp. |
| `Bytes.concat(...bytes) -> bytes` | Join buffers in order. |
| `Bytes.indexOf(b: bytes, needle: bytes, from: integer?) -> integer?` | 1-based position of the first occurrence, or `nil`. An empty needle matches at `from`. |
| `Bytes.toHex(b: bytes) -> string` | Lowercase hex, two characters per byte. This is how a buffer gets looked at — `print` shows only a length — and how one becomes a table key, since `bytes` is not keyable itself. |
| `Bytes.fromHex(s: string) -> bytes?` | Parse hex. `nil` for an odd length or any non-hex character, rather than a buffer that is quietly half right. |

Two things `bytes` deliberately does **not** do:

* **`..` does not accept it.** Concatenation builds a string, and a buffer that
  is not text has no string form. Use `Bytes.concat`, or `Bytes.toHex` to show
  one. `print(b)` writes `<bytes len=N>` for the same reason.
* **It cannot be a table key.** Rare enough not to be worth the cost on the
  table's hot path; key by `Bytes.toHex(b)` instead.

Equality is by contents, and a `bytes` is never equal to a `string`.

```saule
local raw: bytes = Bytes.of(0x00, 0x7f, 0x80, 0xc3, 0x28, 0xff)

println(#raw)                               -- 6
println(Bytes.toHex(raw))                   -- 007f80c328ff
println(Bytes.byte(raw, 3) ?? -1)           -- 128
println(Bytes.toString(raw) ?? "not text")  -- not text

-- The distinction the two types exist for:
local sun: bytes = Bytes.fromString("saulė")
println(#sun)                               -- 6 bytes
println(String.len("saulė"))                -- 5 codepoints

println(Bytes.toHex(Bytes.sub(raw, 2, 4)))  -- 7f80c3
println(Bytes.indexOf(raw, Bytes.of(0x80)) ?? 0)  -- 3
```

---
