---
title: "String"
description: "UTF-8 aware text utilities. Every index is 1-based and counted in"
sidebar:
  order: 2
---

<!-- Generated from DOCS.md by `npm run sync-docs`. Edit that file, not this one. -->

UTF-8 aware text utilities. Every index is **1-based** and counted in
**characters**, not bytes, so `String.sub` can never cut a multi-byte
character in half. A negative index counts from the end, where `-1` is the
last character.

Saule has **no pattern language**. Every function here operates on literal
text: the `.` in `String.replace(s, ".", "-")` is a full stop, not "any
character".

### Characters and codepoints

<div class="api-list">
<details class="api-entry" id="string-byte">
<summary><code class="api-sig"><span class="sig-fn">String.byte</span><span class="sig-punct">(</span><span class="sig-param">s</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">,</span> <span class="sig-param">i</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">?</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">integer</span><span class="sig-punct">?</span></code></summary>


The Unicode codepoint of the `i`-th character, or `nil` when `i` is outside
the string. `i` defaults to 1, so a bare `String.byte(s)` reads the first
character.

Nullable on purpose: a scan can walk off the end and stop, rather than
needing a length check in front of every read.

```saule
local word: string = "Saule"

println(String.byte(word) ?? 0)         -- 83, the codepoint of "S"
println(String.byte(word, 2) ?? 0)      -- 97, the codepoint of "a"
println(String.byte(word, 99) == nil)   -- true, past the end

-- Walking a string codepoint by codepoint, stopping at the end.
local i: integer = 1
while String.byte(word, i) != nil do
    println(i .. ": " .. (String.byte(word, i) ?? 0))
    i = i + 1
end
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="string-char">
<summary><code class="api-sig"><span class="sig-fn">String.char</span><span class="sig-punct">(</span><span class="sig-punct">...</span><span class="sig-type">integer</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">string</span></code></summary>


Builds a string from codepoints. The inverse of `String.byte`.

Note what this is **not**: `String.char(0x80)` is the two-byte UTF-8
encoding of U+0080, because the result is text. For the single byte `0x80`,
reach for [`Bytes.of`](/saule/stdlib/bytes/).

```saule
println(String.char(72, 105))           -- Hi
println(String.char(83, 97, 117, 108, 101))  -- Saule

-- Codepoints past ASCII work the same way.
println(String.char(0x160))             -- Š

-- Round trip, through `byte` and back.
local source: string = "Hi"
println(String.char(String.byte(source, 1) ?? 0, String.byte(source, 2) ?? 0))  -- Hi
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="string-iter">
<summary><code class="api-sig"><span class="sig-fn">String.iter</span><span class="sig-punct">(</span><span class="sig-param">s</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-kw">fn</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-punct">(</span><span class="sig-type">string</span><span class="sig-punct">?</span><span class="sig-punct">,</span> <span class="sig-type">integer</span><span class="sig-punct">?</span><span class="sig-punct">)</span></code></summary>


A step closure over the characters of `s`, yielding each character and its
1-based index. Written for `for`, which stops when the closure answers `nil`.

```saule
for ch, i in String.iter("hey") do
    println(i .. ": " .. ch)     -- 1: h, then 2: e, then 3: y
end

-- Counting characters that match something.
local vowels: integer = 0
for ch in String.iter("saule") do
    if String.contains("aeiou", ch) then
        vowels = vowels + 1
    end
end
println(vowels)                  -- 3
```


</details>
</div>

### Length and slicing

<div class="api-list">
<details class="api-entry" id="string-len">
<summary><code class="api-sig"><span class="sig-fn">String.len</span><span class="sig-punct">(</span><span class="sig-param">s</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">integer</span></code></summary>


The number of **characters**, which is not the number of bytes. `#s` is not
defined for a string — use this, and use `#Bytes.fromString(s)` when the
byte count is what you want.

```saule
local word: string = "saulė"

println(String.len(word))                  -- 5 characters
println(#Bytes.fromString(word))           -- 6 bytes: "ė" takes two

-- Which is why slicing is by character, not by byte.
println(String.sub(word, 5, 5))            -- ė
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="string-sub">
<summary><code class="api-sig"><span class="sig-fn">String.sub</span><span class="sig-punct">(</span><span class="sig-param">s</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">,</span> <span class="sig-param">from</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">,</span> <span class="sig-param">to</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">?</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">string</span></code></summary>


The substring from `from` to `to`, both inclusive and 1-based. `to` defaults
to the end of the string. Either index may be negative to count from the
end, and both clamp, so an out-of-range slice is empty rather than an error.

```saule
local word: string = "Saule"

println(String.sub(word, 2, 4))     -- aul
println(String.sub(word, 3))        -- ule, to the end
println(String.sub(word, -3))       -- ule, the last three
println(String.sub(word, -3, -2))   -- ul
println(String.sub(word, 10, 20))   -- (empty, clamped)

-- Trimming a known prefix.
local path: string = "src/main.sau"
if String.starts(path, "src/") then
    println(String.sub(path, 5))    -- main.sau
end
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="string-rep">
<summary><code class="api-sig"><span class="sig-fn">String.rep</span><span class="sig-punct">(</span><span class="sig-param">s</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">,</span> <span class="sig-param">n</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">string</span></code></summary>


`s` repeated `n` times, with nothing between the copies. `n` of zero or less
gives the empty string.

```saule
println(String.rep("ab", 3))        -- ababab
println(String.rep("-", 20))        -- --------------------
println(String.rep("x", 0))         -- (empty)

-- A quick ruler under a heading.
local title: string = "Report"
println(title)
println(String.rep("=", String.len(title)))   -- ======
```


</details>
</div>

### Case

<div class="api-list">
<details class="api-entry" id="string-lower">
<summary><code class="api-sig"><span class="sig-fn">String.lower</span><span class="sig-punct">(</span><span class="sig-param">s</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">string</span></code></summary>


Lowercases `s`. Unicode-aware, not ASCII-only: `Š` folds to `š` and `Ė` to
`ė`, so this is correct for text in any language the string can hold.

```saule
println(String.lower("SAULE"))        -- saule
println(String.lower("Mixed Case"))   -- mixed case
println(String.lower("ŠALTIS"))       -- šaltis, past ASCII too

-- Case-insensitive comparison, ASCII only.
local typed: string = "YES"
println(String.lower(typed) == "yes")   -- true
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="string-upper">
<summary><code class="api-sig"><span class="sig-fn">String.upper</span><span class="sig-punct">(</span><span class="sig-param">s</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">string</span></code></summary>


Uppercases `s`, Unicode-aware in the same way as `String.lower`.

```saule
println(String.upper("saule"))        -- SAULE
println(String.upper("Mixed Case"))   -- MIXED CASE
println(String.upper("saulė"))        -- SAULĖ

-- Initial-capitalising a word.
local name: string = "ada"
println(String.upper(String.sub(name, 1, 1)) .. String.sub(name, 2))   -- Ada
```


</details>
</div>

### Searching

<div class="api-list">
<details class="api-entry" id="string-find">
<summary><code class="api-sig"><span class="sig-fn">String.find</span><span class="sig-punct">(</span><span class="sig-param">s</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">,</span> <span class="sig-param">needle</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">,</span> <span class="sig-param">from</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">?</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-punct">(</span><span class="sig-type">integer</span><span class="sig-punct">?</span><span class="sig-punct">,</span> <span class="sig-type">integer</span><span class="sig-punct">?</span><span class="sig-punct">)</span></code></summary>


The 1-based start and end indices of the first occurrence of `needle`, or
`nil, nil` when there is none. `from` says where to start looking and
defaults to 1.

Two values, so you can slice around the match without measuring it. For just
the start index, [`String.indexOf`](/saule/stdlib/string/#string-indexof) answers the same
question with one value.

```saule
local line: string = "name = Saule"

local s: integer?, e: integer? = String.find(line, " = ")
if s != nil and e != nil then
    println(String.sub(line, 1, s! - 1))   -- name
    println(String.sub(line, e! + 1))      -- Saule
end

-- A miss answers nil for both.
local missS: integer?, missE: integer? = String.find(line, "zzz")
println(missS == nil and missE == nil)     -- true

-- `from` walks through every occurrence. `String.indexOf` is the one-value
-- form, which is what a loop like this wants. The match is declared *inside*
-- the loop: a nullable narrowed by `!= nil` stays narrowed for the rest of
-- the block, so it cannot be assigned a nullable again.
local text: string = "a,b,c"
local start: integer = 1
while true do
    local at: integer? = String.indexOf(text, ",", start)
    if at == nil then
        break
    end
    println("comma at " .. at!)            -- 2, then 4
    start = at! + 1
end
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="string-indexof">
<summary><code class="api-sig"><span class="sig-fn">String.indexOf</span><span class="sig-punct">(</span><span class="sig-param">s</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">,</span> <span class="sig-param">needle</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">,</span> <span class="sig-param">from</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">?</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">integer</span><span class="sig-punct">?</span></code></summary>


The 1-based character index where `needle` first appears, or `nil` when it
does not. `from` says where to start and defaults to 1.

[`String.find`](/saule/stdlib/string/#string-find) answers the same question and also gives the
end index.

```saule
println(String.indexOf("hello", "ll") ?? 0)      -- 3
println(String.indexOf("hello", "z") == nil)     -- true
println(String.indexOf("a,b,a", "a", 2) ?? 0)    -- 5, searching from 2

-- Splitting on the first separator only.
local setting: string = "path=/usr/local/bin"
local at: integer? = String.indexOf(setting, "=")
if at != nil then
    println(String.sub(setting, 1, at! - 1))     -- path
    println(String.sub(setting, at! + 1))        -- /usr/local/bin
end
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="string-contains">
<summary><code class="api-sig"><span class="sig-fn">String.contains</span><span class="sig-punct">(</span><span class="sig-param">s</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">,</span> <span class="sig-param">needle</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">boolean</span></code></summary>


Whether `needle` appears anywhere in `s`. An empty needle is always present.

```saule
println(String.contains("Saule", "aul"))   -- true
println(String.contains("Saule", "xyz"))   -- false

-- Filtering a list of names.
local names: table<string> = {"ada.sau", "readme.md", "main.sau"}
for name in names do
    if String.contains(name, ".sau") then
        println(name)                      -- ada.sau, then main.sau
    end
end
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="string-starts">
<summary><code class="api-sig"><span class="sig-fn">String.starts</span><span class="sig-punct">(</span><span class="sig-param">s</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">,</span> <span class="sig-param">prefix</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">boolean</span></code></summary>


Whether `s` begins with `prefix`. An empty prefix is always present.

```saule
println(String.starts("Saule", "Sau"))    -- true
println(String.starts("Saule", "aul"))    -- false, not at the start
println(String.starts("Saule", ""))       -- true

-- Routing on a prefix.
local arg: string = "--verbose"
if String.starts(arg, "--") then
    println("long flag: " .. String.sub(arg, 3))   -- long flag: verbose
end
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="string-ends">
<summary><code class="api-sig"><span class="sig-fn">String.ends</span><span class="sig-punct">(</span><span class="sig-param">s</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">,</span> <span class="sig-param">suffix</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">boolean</span></code></summary>


Whether `s` ends with `suffix`. An empty suffix is always present.

```saule
println(String.ends("main.sau", ".sau"))   -- true
println(String.ends("main.sau", ".lua"))   -- false

-- Picking a handler by file extension.
local file: string = "notes.md"
if String.ends(file, ".md") then
    println("markdown")
elseif String.ends(file, ".sau") then
    println("saule")
end
```


</details>
</div>

### Splitting

<div class="api-list">
<details class="api-entry" id="string-split">
<summary><code class="api-sig"><span class="sig-fn">String.split</span><span class="sig-punct">(</span><span class="sig-param">s</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">,</span> <span class="sig-param">sep</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">string</span><span class="sig-punct">&gt;</span></code></summary>


Splits `s` on every occurrence of `sep`. The result always holds one more
piece than there were separators, so splitting the empty string gives one
empty piece rather than none. An empty `sep` splits into single characters.

```saule
local parts: table<string> = String.split("id,name,score", ",")
println(#parts)                       -- 3
println(parts[2] ?? "")               -- name

-- The piece count is always separators + 1.
println(#String.split("a,,b", ","))   -- 3: "a", "", "b"
println(#String.split("", ","))       -- 1, one empty piece

-- An empty separator gives characters.
println(#String.split("abc", ""))     -- 3

-- Parsing a simple key=value line.
local pair: table<string> = String.split("colour=amber", "=")
println((pair[1] ?? "") .. " is " .. (pair[2] ?? ""))   -- colour is amber
```


</details>
</div>

### Joining

<div class="api-list">
<details class="api-entry" id="string-join">
<summary><code class="api-sig"><span class="sig-fn">String.join</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">(</span><span class="sig-param">sep</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">,</span> <span class="sig-param">parts</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">V</span><span class="sig-punct">&gt;</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">string</span></code></summary>


Joins `parts` with `sep` between them, rendering each element the way
`tostring` would. The same operation as
[`Table.concat`](/saule/stdlib/table/#table-concat), with the arguments the
other way round so the call reads as a sentence.

```saule
println(String.join(", ", {"a", "b", "c"}))   -- a, b, c
println(String.join(" | ", {1, 2, 3}))        -- 1 | 2 | 3
println(String.join("", {"a", "b"}))          -- ab

-- Round-tripping a split.
local csv: string = "x,y,z"
println(String.join(",", String.split(csv, ",")) == csv)   -- true
```


</details>
</div>

### Trimming

<div class="api-list">
<details class="api-entry" id="string-trim">
<summary><code class="api-sig"><span class="sig-fn">String.trim</span><span class="sig-punct">(</span><span class="sig-param">s</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">string</span></code></summary>


Removes whitespace from both ends. Unicode whitespace, not only the ASCII
space — a non-breaking space goes too.

```saule
println("[" .. String.trim("   hi   ") .. "]")    -- [hi]
println("[" .. String.trim("\t hi \n") .. "]")     -- [hi]
println("[" .. String.trim("   ") .. "]")         -- []

-- Cleaning a line of input before testing it.
local typed: string = "  yes  "
println(String.trim(typed) == "yes")              -- true
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="string-trimstart">
<summary><code class="api-sig"><span class="sig-fn">String.trimStart</span><span class="sig-punct">(</span><span class="sig-param">s</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">string</span></code></summary>


Removes whitespace from the start only.

```saule
println("[" .. String.trimStart("  hi  ") .. "]")   -- [hi  ]

-- Measuring indentation.
local line: string = "    indented"
println(String.len(line) - String.len(String.trimStart(line)))   -- 4
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="string-trimend">
<summary><code class="api-sig"><span class="sig-fn">String.trimEnd</span><span class="sig-punct">(</span><span class="sig-param">s</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">string</span></code></summary>


Removes whitespace from the end only.

```saule
println("[" .. String.trimEnd("  hi  ") .. "]")   -- [  hi]

-- Dropping a trailing newline without touching leading indentation.
local line: string = "  code\n"
println("[" .. String.trimEnd(line) .. "]")       -- [  code]
```


</details>
</div>

### Replacing

<div class="api-list">
<details class="api-entry" id="string-replace">
<summary><code class="api-sig"><span class="sig-fn">String.replace</span><span class="sig-punct">(</span><span class="sig-param">s</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">,</span> <span class="sig-param">from</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">,</span> <span class="sig-param">to</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">,</span> <span class="sig-param">limit</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">?</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">string</span></code></summary>


Replaces every occurrence of `from` with `to`, or only the first `limit` of
them when `limit` is given. Literal text, not a pattern. An empty `from`
matches nothing and returns `s` unchanged, so this can never loop forever.

```saule
println(String.replace("a.b.c", ".", "/"))        -- a/b/c
println(String.replace("a.b.c", ".", "/", 1))     -- a/b.c, first only
println(String.replace("aaa", "a", ""))           -- (empty), deleting
println(String.replace("abc", "", "-"))           -- abc, unchanged

-- The `.` is a full stop, not "any character".
println(String.replace("a1b", ".", "!"))          -- a1b, nothing matched
```


</details>
</div>

### Padding

<div class="api-list">
<details class="api-entry" id="string-padstart">
<summary><code class="api-sig"><span class="sig-fn">String.padStart</span><span class="sig-punct">(</span><span class="sig-param">s</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">,</span> <span class="sig-param">width</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">,</span> <span class="sig-param">fill</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">?</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">string</span></code></summary>


Pads `s` on the **left** until it is `width` characters long. `fill` defaults
to a single space and repeats if it is longer than one character. Never
truncates: a string already at or past `width` comes back unchanged.

```saule
println(String.padStart("7", 3, "0"))        -- 007
println(String.padStart("7", 3))             -- "  7"
println(String.padStart("toolong", 3, "0"))  -- toolong, never cut

-- Right-aligning a column of numbers.
for n in {5, 42, 7} do
    println(String.padStart(tostring(n), 4, " "))   --    5,   42,    7
end
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="string-padend">
<summary><code class="api-sig"><span class="sig-fn">String.padEnd</span><span class="sig-punct">(</span><span class="sig-param">s</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">,</span> <span class="sig-param">width</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">,</span> <span class="sig-param">fill</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">?</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">string</span></code></summary>


The same, padding on the **right**.

```saule
println(String.padEnd("7", 3, "0"))      -- 700
println(String.padEnd("hp", 6) .. "|")   -- hp    |

-- Laying out a two-column table without `String.format`.
local rows: table<string> = {"name", "id"}
for key in rows do
    println(String.padEnd(key, 8) .. ": set")   -- name    : set, id      : set
end
```


</details>
</div>

### Formatting

<div class="api-list">
<details class="api-entry" id="string-format">
<summary><code class="api-sig"><span class="sig-fn">String.format</span><span class="sig-punct">(</span><span class="sig-param">fmt</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">,</span> <span class="sig-punct">...</span><span class="sig-type">any</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">string</span></code></summary>


C-style formatting. The specifiers are `%d` and `%i` (integer), `%f` and
`%g` (float), `%s` (any value, rendered as `tostring` would), `%x` and `%X`
(hexadecimal), `%o` (octal), `%c` (a character from a codepoint) and `%%`
for a literal percent sign.

Width, precision and flags work as they do in C: `%-5s` left-aligns in five
columns, `%05.1f` pads with zeros to five columns with one decimal place.

```saule
println(String.format("%d items", 42))              -- 42 items
println(String.format("%.2f", 3.14159))             -- 3.14
println(String.format("%-6s|", "hp"))               -- hp    |
println(String.format("%06.2f", 3.14159))           -- 003.14
println(String.format("%x and %X", 255, 255))       -- ff and FF
println(String.format("%s scored %d%%", "Ada", 98)) -- Ada scored 98%

-- `%s` takes anything, including a class that implements `OpToString`.
println(String.format("[%s] [%s]", true, 1.5))      -- [true] [1.5]
```

</details>
</div>

