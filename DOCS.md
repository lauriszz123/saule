# Saule Standard Library

This is the reference for everything Saule ships out of the box — the prelude
(`print`, `assert`, …) plus the seven static-class modules (`String`, `Math`,
`Table`, `pairs`/`ipairs`, `Io` / `File`, `Os`).

For language syntax see [README.md](./README.md). For the runtime side of
errors / pattern matching / generics see the corresponding README sections.

> All function and method signatures are written in Saule's own type notation.
> `T?` is nullable, `(A, B)` is a multi-return tuple, `...T` is variadic.

---

## Table of Contents

- [Prelude (always in scope)](#prelude-always-in-scope)
- [`String`](#string)
- [`Math`](#math)
- [`Table`](#table)
- [`pairs` / `ipairs`](#pairs--ipairs)
- [`Io` / `File`](#io--file)
- [`Os`](#os)

---

## Prelude (always in scope)

These names are bound at the top of every module — no import required.

| Signature | Description |
| --- | --- |
| `print(...any) -> nil` | Write arguments to stdout separated by tabs, no trailing newline. |
| `println(...any) -> nil` | Same as `print`, with a trailing newline. |
| `printf(fmt: string, ...any) -> nil` | Format like `String.format` and write to stdout (no newline). |
| `tostring(v: any) -> string` | Human-readable rendering of any value. |
| `type(v: any) -> string` | Returns the runtime type name: `"integer"`, `"float"`, `"string"`, `"boolean"`, `"nil"`, `"function"`, `"table"`, or the class name for instances. |
| `int(n: integer \| float) -> integer` | Truncating conversion (`int(3.9) == 3`). |
| `float(n: integer \| float) -> float` | Lossless widening (`float(3) == 3.0`). |
| `assert(v: any, msg: any?) -> any` | If `v` is truthy, returns it; otherwise throws `msg` (default `"assertion failed"`). |
| `error(msg: string) -> nil` | Throws `msg` as a runtime error. Equivalent to `throw msg`. |

```saule
local n: integer = assert(int("42"!), "expected an integer")
printf("got %d\n", n)
```

---

## `String`

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

## `Math`

Numeric utilities. `integer` and `float` are distinct in Saule, so several
functions return `integer` even when given a `float` (e.g. `floor`, `ceil`).

### Constants

| Name | Value |
| --- | --- |
| `Math.pi` | π |
| `Math.e` | Euler's number |
| `Math.huge` | `+inf` |
| `Math.maxinteger` | i64 max |
| `Math.mininteger` | i64 min |

### Rounding & conversion

| Signature | Description |
| --- | --- |
| `Math.floor(n: number) -> integer` | Round toward `-inf`. |
| `Math.ceil(n: number) -> integer` | Round toward `+inf`. |
| `Math.round(n: number) -> integer` | Banker's-friendly round. |
| `Math.tointeger(v: any) -> integer?` | Lossless integer conversion (`3.0` → `3`, `3.5` → `nil`). |
| `Math.type(v: any) -> string?` | `"integer"` / `"float"` for numbers, `nil` for non-numbers. |
| `Math.sign(n: number) -> integer` | `-1`, `0`, or `1`. |

### Arithmetic

| Signature | Description |
| --- | --- |
| `Math.abs(n: number) -> number` | Magnitude (returns same kind as input). |
| `Math.min(...number) -> number` | Minimum of all arguments. |
| `Math.max(...number) -> number` | Maximum of all arguments. |
| `Math.clamp(n: number, lo: number, hi: number) -> number` | Pin `n` into `[lo, hi]`. |
| `Math.fmod(a: number, b: number) -> number` | Truncated remainder. |
| `Math.modf(n: float) -> table<any>` | `[int_part, frac_part]`. |
| `Math.pow(a: number, b: number) -> number` | `a^b`. |
| `Math.sqrt(n: number) -> float` | √n. |
| `Math.exp(n: number) -> float` | `e^n`. |
| `Math.log(n: number, base: number?) -> float` | Natural log; with `base`, log in that base. |

### Trigonometry

`sin`, `cos`, `tan`, `asin`, `acos`, `atan(y, x?)` (`atan2`-style),
`deg(rad)`, `rad(deg)`. All take/return floats.

### Random & bit-ish

| Signature | Description |
| --- | --- |
| `Math.random() -> float` | Uniform `[0, 1)`. |
| `Math.random(n: integer) -> integer` | Uniform `[1, n]`. |
| `Math.random(lo: integer, hi: integer) -> integer` | Uniform `[lo, hi]`. |
| `Math.randomseed(seed: integer) -> nil` | Reset the PRNG. |
| `Math.ult(a: integer, b: integer) -> boolean` | Unsigned less-than. |

```saule
Math.randomseed(42)
local roll: integer = Math.random(1, 6)
println(Math.sqrt(2.0), Math.log(8.0, 2.0))     -- 1.414…  3.0
```

---

## `Table`

Tables in Saule double as arrays (integer-keyed, 1-based) and maps. These
helpers operate on the array side; map operations are just `t[k]` / `t[k] = v`.

| Signature | Description |
| --- | --- |
| `Table.insert(t: table<any>, value: any, pos: integer?) -> nil` | Append by default, or insert at `pos` shifting elements right. |
| `Table.remove(t: table<any>, pos: integer?) -> any?` | Pop from the end by default, or remove at `pos` shifting left. Returns the removed value (or `nil`). |
| `Table.sort(t: table<any>, cmp: fn(any, any): boolean) -> nil` | Sort in place; `cmp(a, b)` should return `true` when `a` precedes `b`. |
| `Table.concat(t: table<any>, sep: string?, from: integer?, to: integer?) -> string` | Join string/number elements with `sep` (default `""`). |

```saule
local xs: table<integer> = {3, 1, 4, 1, 5}
Table.sort(xs, fn(a, b) => a < b)
println(Table.concat(xs, ", "))                 -- 1, 1, 3, 4, 5
local last: integer = Table.remove(xs)!         -- 5
```

---

## `pairs` / `ipairs`

Free iterator helpers usable with `for ... in`. Both return a step closure;
the loop reads it until it returns `nil`.

| Signature | Description |
| --- | --- |
| `pairs(t: table<any>) -> fn(): (any?, any?)` | Iterate every `(key, value)` pair in unspecified order. |
| `ipairs(t: table<any>) -> fn(): (integer?, any?)` | Iterate integer keys `1, 2, 3, …` until the first gap. |

```saule
local scores: table<string, integer> = {}
scores["alice"] = 10
scores["bob"]   = 7
for name, n in pairs(scores) do
    println(name, n)
end
```

---

## `Io` / `File`

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

## `Os`

Process, environment, time, and filesystem primitives. Filesystem calls
return a `boolean` on success/failure (no detailed error type yet — wrap
in `assert` if you want to abort).

### Constants

| Name | Type | Description |
| --- | --- | --- |
| `Os.sep` | `string` | Path separator (`"/"` or `"\\"`). |
| `Os.lineSep` | `string` | Line ending (`"\n"` or `"\r\n"`). |

### Time

| Signature | Description |
| --- | --- |
| `Os.time() -> integer` | Unix epoch in seconds. |
| `Os.clock() -> float` | CPU time used by the process, in seconds. |
| `Os.difftime(t2: integer, t1: integer) -> integer` | `t2 - t1`. |
| `Os.date(format: string?, time: integer?) -> string` | `strftime`-style. Default format `"%c"`, default time = now. |
| `Os.sleep(seconds: number) -> nil` | Floats allowed for sub-second sleeps. |

### Environment

| Signature | Description |
| --- | --- |
| `Os.getenv(name: string) -> string?` | `nil` if unset. |
| `Os.setenv(name: string, value: string) -> nil` | Set for the current process. |
| `Os.cwd() -> string` | Absolute path of the working directory. |
| `Os.chdir(path: string) -> boolean` | Returns `false` on error. |

### Filesystem

| Signature | Description |
| --- | --- |
| `Os.exists(path: string) -> boolean` | True for files and directories. |
| `Os.list(path: string) -> table<string>` | Children of a directory (names only). |
| `Os.mkdir(path: string, recursive: boolean?) -> boolean` | Create a directory; `recursive` defaults to `false`. |
| `Os.remove(path: string) -> boolean` | Remove a file or empty directory. |
| `Os.rename(from: string, to: string) -> boolean` | Move/rename. |
| `Os.tmpname() -> string` | Path to a fresh temp file under the OS temp dir. |

### Process

| Signature | Description |
| --- | --- |
| `Os.pid() -> integer` | This process's PID. |
| `Os.args() -> table<string>` | CLI args passed to the script. |
| `Os.execute(cmd: string) -> integer` | Run via the shell; returns exit code. |
| `Os.exit(code: integer?) -> nil` | Terminate the process. |
| `Os.platform() -> OsPlatform` | Current OS as an enum (`.value` is `"linux"` / `"macos"` / `"windows"` / `"other"`). |

### `OsPlatform` variants

`Linux` (`"linux"`), `Macos` (`"macos"`), `Windows` (`"windows"`), `Other` (`"other"`).

```saule
local started: float = Os.clock()
Os.sleep(0.05)
printf("elapsed: %.3fs\n", Os.clock() - started)

if Os.platform().value == "linux" then
    println("home: " .. (Os.getenv("HOME") ?? "?"))
end
```

---

## Conventions

A handful of patterns repeat across the stdlib:

- **1-based indexing everywhere.** `String.sub`, `String.iter`, `Table.*`,
  array literals — all 1-based. Negative indices count from the end where
  it makes sense.
- **`?` means "may not happen".** Lookups that can miss (`String.find`,
  `Io.open`, `Os.getenv`) return nullable types so the typechecker forces
  you to handle the absence with `?.`, `??`, `!`, or a `match`.
- **Multi-return is a tuple.** Functions like `String.find` return
  `(integer?, integer?)`. Destructure with `local s, e = ...` or pattern-match.
- **No exceptions for "expected" failures.** Filesystem helpers return
  `boolean` rather than throwing. Use `throw` / `try` / `catch` for genuinely
  exceptional cases — see [README §Error Handling](./README.md#error-handling).

