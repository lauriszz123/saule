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
| `String.iter(s: string) -> fn() -> (string?, integer?)` | Step closure usable in `for c, i in String.iter(s) do ... end`. |

### Splitting, joining, trimming

Saule has no pattern language. These operate on **literal text** — the `.` in
`String.replace(s, ".", "-")` is a full stop, not "any character".

| Signature | Description |
| --- | --- |
| `String.split(s: string, sep: string) -> table<string>` | Split on each occurrence of `sep`. An empty `sep` splits into characters. Splitting `""` gives one empty piece, so `#parts` is always `occurrences + 1`. |
| `String.join<V>(sep: string, parts: table<V>) -> string` | Join `parts` with `sep`, each element rendered as `tostring` would. Same operation as `Table.concat(parts, sep)`, argument order reversed to read as a sentence. |
| `String.trim(s: string) -> string` | Drop leading and trailing whitespace (Unicode, not just ASCII spaces). |
| `String.trimStart(s: string) -> string` | Leading whitespace only. |
| `String.trimEnd(s: string) -> string` | Trailing whitespace only. |
| `String.replace(s: string, from: string, to: string, limit: integer?) -> string` | Replace every occurrence of `from`, or the first `limit` of them. An empty `from` matches nothing and returns `s` unchanged. |
| `String.contains(s: string, needle: string) -> boolean` | Substring test. |
| `String.indexOf(s: string, needle: string, from: integer?) -> integer?` | 1-based character index of the first match, or `nil`. `String.find` answers the same question with an end index too. |
| `String.padStart(s: string, width: integer, fill: string?) -> string` | Pad on the left to `width` characters; `fill` defaults to `" "` and repeats if longer than one character. Never truncates. |
| `String.padEnd(s: string, width: integer, fill: string?) -> string` | The same, padding on the right. |

```saule
for ch, i in String.iter("hey") do
    printf("%d:%s ", i, ch)
end
println()                                       -- 1:h 2:e 3:y
println(String.format("%-8s %05d", "hp", 42))   -- hp       00042

local fields: table<string> = String.split("id,name,score", ",")
println(String.join(" | ", fields))             -- id | name | score
println(String.replace("a.b.c", ".", "/"))      -- a/b/c
println(String.padStart("7", 3, "0"))           -- 007
println(String.indexOf("hello", "ll") ?? 0)     -- 3
```

---
