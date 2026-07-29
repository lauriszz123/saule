---
title: "String"
description: "UTF-8 aware string utilities. Indices are 1-based and may be negative (-1 is the last character) — see String.sub / String.find."
sidebar:
  order: 2
---

<!-- Generated from DOCS.md by `npm run sync-docs`. Edit that file, not this one. -->

UTF-8 aware string utilities. Indices are **1-based** and may be negative
(`-1` is the last character) — see `String.sub` / `String.find`.

| Signature | Description |
| --- | --- |
| `String.byte(s: string, i: integer?) -> integer?` | Codepoint of the `i`-th character (default 1), or `nil` if out of range. |
| `String.char(...integer) -> string` | Build a string from codepoints: `String.char(72, 105) == "Hi"`. |
| `String.format(fmt: string, ...any) -> string` | C-style format spec: `%d`, `%i`, `%f`, `%g`, `%s`, `%x`, `%X`, `%o`, `%c`, `%%`. Width/precision/flags supported. |
| `String.len(s: string) -> integer` | Character count (not bytes). |
| `String.sub(s: string, from: integer, to: integer?) -> string` | Substring; `to` defaults to the end. Negatives count from the end. |
| `String.rep(s: string, n: integer) -> string` | `String.rep("ab", 3) == "ababab"`. |
| `String.starts(s: string, prefix: string) -> boolean` | Prefix test. |
| `String.ends(s: string, suffix: string) -> boolean` | Suffix test. |
| `String.find(s: string, needle: string, from: integer?) -> (integer?, integer?)` | First match's `(start, end)` indices, both `nil` on miss. |
| `String.lower(s: string) -> string` | ASCII lowercasing. |
| `String.upper(s: string) -> string` | ASCII uppercasing. |
| `String.iter(s: string) -> fn(): (string?, integer?)` | Step closure usable in `for c, i in String.iter(s) do ... end`. |

```saule
for ch, i in String.iter("hey") do
    printf("%d:%s ", i, ch)
end
println()                                       -- 1:h 2:e 3:y
println(String.format("%-8s %05d", "hp", 42))   -- hp       00042
```

---
