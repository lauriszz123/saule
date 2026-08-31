# Saule Standard Library

This is the reference for everything Saule ships out of the box — the prelude
(`print`, `assert`, …) plus the static-class modules (`String`, `Math`,
`Table`, `Io` / `File`, `Os`, `Saule`).

For language syntax see [README.md](./README.md). For the runtime side of
errors / pattern matching / generics see the corresponding README sections.

> All function and method signatures are written in Saule's own type notation.
> `T?` is nullable, `(A, B)` is a multi-return tuple, `...T` is variadic, and
> `fn(A) -> R` is a callback slot — there is no bare `function` type.

---

## Table of Contents

- [Prelude (always in scope)](#prelude-always-in-scope)
- [`String`](#string)
- [`Math`](#math)
- [`Table`](#table)
- [`Iter`](#iter)
- [`Io` / `File`](#io--file)
- [`Os`](#os)
- [`Saule`](#saule)

---

## Prelude (always in scope)

These names are bound at the top of every module — no import required.

| Signature | Description |
| --- | --- |
| `print(...any) -> nil` | Write arguments to stdout separated by tabs, no trailing newline. |
| `println(...any) -> nil` | Same as `print`, with a trailing newline. |
| `printf(fmt: string, ...any) -> nil` | Format like `String.format` and write to stdout (no newline). |
| `tostring(v: any) -> string` | Human-readable rendering of any value. For a class instance implementing `OpToString`, calls its `toString()`; `print` / `println` / `..` render the same way. |
| `type(v: any) -> string` | Returns the runtime type name: `"integer"`, `"float"`, `"string"`, `"boolean"`, `"nil"`, `"function"`, `"table"`, or the class name for instances. `"function"` is a runtime tag covering every callable — it is not a type you can write, since a function's type is its signature (`fn(A) -> R`). |
| `assert<T>(v: T?, msg: string?) -> T` | If `v` is truthy, returns it with its nullability stripped; otherwise throws `msg` (default `"assertion failed"`). |
| `error(msg: string) -> nil` | Throws `msg` as a runtime error. Equivalent to `throw msg`. |

```saule
local n: integer = "42" as integer ?? 0
printf("got %d\n", n)
```

Conversion between types is the `as` cast, not a function — `3.9 as
integer`, `n as string`, `"42" as integer` (which is `integer?`, since the
text may hold no number). See [README §Casting](./README.md#casting).

### Prelude interfaces

These interfaces are in scope everywhere too, so a class can implement them
without an import. They carry no behaviour of their own — each one is a
contract the language itself looks for.

| Interface | What it enables |
| --- | --- |
| `Iterable<T>` | `for v in instance do` — `fn iter() -> fn() -> T?` returns the step closure. |
| `Iterable2<K, V>` | `for k, v in instance do` — the step closure returns two values. |
| `OpAdd<T, R>` `OpSub<T, R>` `OpMul<T, R>` `OpDiv<T, R>` `OpMod<T, R>` `OpPow<T, R>` | `+` `-` `*` `/` `%` `^` — `fn add(other: T) -> R`, and so on. |
| `OpBAnd<T, R>` `OpBOr<T, R>` `OpBXor<T, R>` `OpShl<T, R>` `OpShr<T, R>` | `&` `\|` `~` `<<` `>>` — `fn band(other: T) -> R`, and so on. |
| `OpNeg<R>` | `-a` — `fn neg() -> R`. |
| `OpBNot<R>` | `~a` — `fn bnot() -> R`. |
| `OpLen` | `#a` — `fn len() -> integer`. |
| `OpConcat<T, R>` | `a .. b` — `fn concat(other: T) -> R`. |
| `OpEq<T>` | `a == b` and `a != b` — `fn equals(other: T) -> boolean`. |
| `OpCompare<T>` | `<` `<=` `>` `>=` — `fn compare(other: T) -> integer`, negative / zero / positive. |
| `OpToString` | `tostring(a)`, `print(a)`, and `..` — `fn toString() -> string`. |
| `OpIndex<K, V>` | `a[k]` — `fn index(key: K) -> V`. Saule's `__index`. |
| `OpNewIndex<K, V>` | `a[k] = v` — `fn newIndex(key: K, value: V) -> nil`. Saule's `__newindex`. |
| `Assignable<T>` | `local a: C = t` — `static fn of(value: T) -> C`. A bare `T` may fill a `C` slot; `C.of` builds it. |

The `<T>` on these is a real type argument, checked like any other: `OpAdd`
takes two (the operand and the result), `OpEq` one, `OpLen` none — supplying
the wrong number is an error rather than being ignored. See
[Custom Iterable](./README.md#custom-iterable) and
[Operator Overloading](./README.md#operator-overloading) for the rules and
worked examples, and [Generic Classes](./README.md#generic-classes) for
declaring your own.

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

## `Math`

Numeric utilities. `integer` and `float` are distinct in Saule, so several
functions return `integer` even when given a `float` (e.g. `floor`, `ceil`).

Most of `Math` is **generic over numeric flavour**, written `<N>` below: `N`
is `integer` or `float` and nothing else. So `Math.sqrt(9)` and
`Math.clamp(hp, 0, 100)` are ordinary calls — no cast into them, and none
back out. The functions that *pick* one of their arguments (`abs`, `min`,
`max`, `clamp`, `fmod`) answer in the flavour they were given; the ones that
compute a real number (`sqrt`, `pow`, `log`, the trig family) always answer
`float`; the rounding functions always answer `integer`.

Flavour still cannot be **mixed inside one call**. `Math.max(1, 2.5)` is a
type error for the same reason `1 + 2.5` is — Saule never auto-promotes, and
the stdlib does not get a private exemption. Cast one side with `as float`.

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
| `Math.floor<N>(n: N) -> integer` | Round toward `-inf`. An `integer` comes back unchanged. |
| `Math.ceil<N>(n: N) -> integer` | Round toward `+inf`. |
| `Math.round<N>(n: N) -> integer` | Banker's-friendly round. |
| `Math.type(v: any) -> string?` | `"integer"` / `"float"` for numbers, `nil` for non-numbers. |
| `Math.sign<N>(n: N) -> integer` | `-1`, `0`, or `1`. |

### Arithmetic

| Signature | Description |
| --- | --- |
| `Math.abs<N>(n: N) -> N` | Magnitude, in the flavour it was given. |
| `Math.min<N>(...N) -> N` | Minimum of all arguments, in their shared flavour. |
| `Math.max<N>(...N) -> N` | Maximum of all arguments. |
| `Math.clamp<N>(n: N, lo: N, hi: N) -> N` | Pin `n` into `[lo, hi]`. |
| `Math.fmod<N>(a: N, b: N) -> N` | Truncated remainder, in the flavour it was given. |
| `Math.modf<N>(n: N) -> table<float>` | `[int_part, frac_part]`. |
| `Math.pow<N>(a: N, b: N) -> float` | `a^b`. Always a float — `Math.pow(2, -1)` is `0.5`. |
| `Math.sqrt<N>(n: N) -> float` | √n. |
| `Math.exp<N>(n: N) -> float` | `e^n`. |
| `Math.log<N>(n: N, base: N?) -> float` | Natural log; with `base`, log in that base. |

### Trigonometry

`sin`, `cos`, `tan`, `asin`, `acos`, `atan(y, x?)` (`atan2`-style),
`deg(rad)`, `rad(deg)`. All take an `integer` or a `float` and return a
`float`.

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
println(Math.sqrt(2), Math.log(8, 2))           -- 1.414…  3.0

-- integer in, integer out: no cast either way
local hp: integer = Math.clamp(137, 0, 100)     -- 100
local span: integer = Math.max(3, 7, 2)         -- 7
```

---

## `Table`

Tables in Saule double as arrays (integer-keyed, 1-based) and maps. Most of
these helpers operate on the array side; `keys` / `values` see both, and map
access itself is just `t[k]` / `t[k] = v`.

| Signature | Description |
| --- | --- |
`Table` is the **mutating** half of the sequence API: it changes a table in
place, or answers a question about one. For deriving a *new* sequence from an
existing one — `map`, `filter`, `reduce` — see [`Iter`](#iter). No name means
two different things across the two.

### Changing a table

| Signature | Description |
| --- | --- |
| `Table.insert<V>(t: table<V>, value: V, pos: integer?) -> nil` | Append by default, or insert at `pos` shifting elements right. |
| `Table.remove<V>(t: table<V>, pos: integer?) -> V?` | Pop from the end by default, or remove at `pos` shifting left. Returns the removed value (or `nil`). |
| `Table.sort<V>(t: table<V>, cmp: fn(V, V) -> boolean) -> nil` | Sort in place; `cmp(a, b)` should return `true` when `a` precedes `b`. |
| `Table.reverse<V>(t: table<V>) -> nil` | Reverse the array part in place. `Iter.reverse` returns a new table instead. |
| `Table.clear<V>(t: table<V>) -> nil` | Remove every entry, array and map alike. |

### Reading a table

| Signature | Description |
| --- | --- |
| `Table.len<V>(t: table<V>) -> integer` | Array length — the same number `#t` gives, as a value you can pass around. |
| `Table.contains<V>(t: table<V>, value: V) -> boolean` | Is `value` in the array part? Compares with `==`, so a class implementing `OpEq` is matched by its own rule. |
| `Table.indexOf<V>(t: table<V>, value: V) -> integer?` | 1-based position of the first `==` match, or `nil`. For a *predicate* search use `Iter.findIndex`. |
| `Table.keys<K, V>(t: table<K, V>) -> table<K>` | Every key: array indices `1..#t` in order, then the map's keys in no particular order. |
| `Table.values<K, V>(t: table<K, V>) -> table<V>` | Every value, in the same order as `Table.keys`. |
| `Table.slice<V>(t: table<V>, from: integer, to: integer?) -> table<V>` | New table over the 1-based range; `to` defaults to the end. Negative indices count from the end, and an out-of-range slice is empty rather than an error. |
| `Table.copy<V>(t: table<V>) -> table<V>` | Shallow copy of both halves. Elements are shared, so copying a table of instances gives a new table pointing at the same instances. |
| `Table.concat<V>(t: table<V>, sep: string?, from: integer?, to: integer?) -> string` | Join the elements with `sep` (default `""`), each rendered as `tostring` would. `String.join(sep, t)` is the same operation with the arguments the other way round. |

```saule
local xs: table<integer> = {3, 1, 4, 1, 5}
Table.sort(xs, (a, b) => a < b)
println(Table.concat(xs, ", "))                 -- 1, 1, 3, 4, 5
local last: integer = Table.remove(xs)!         -- 5

println(Table.indexOf(xs, 3) ?? 0)              -- 3
println(Table.concat(Table.slice(xs, -2), " ")) -- 3 4
```

There is no `Table.unpack`. A function returning "however many values the
table holds" has no type Saule can write — the arity is not known until the
call runs — so the table is passed as a table.

---

## `Iter`

Combinators over sequences. Where [`Table`](#table) mutates a table or
answers a question about one, `Iter` derives a **new** sequence and never
writes to its argument.

Every combinator is **eager**: `table` in, `table` out. That is what keeps
them typed — `Iter.map<V, U>(t: table<V>, f: fn(V) -> U)` binds `V` from the
receiver, so a lambda written without annotations still gets real parameter
types, and the result is a real `table<U>`:

```saule
local names: table<string> = Iter.map(users, u => u.name)
local adults: table<User>  = Iter.filter(users, u => u.age >= 18)
local total: integer       = Iter.reduce(users, 0, (acc, u) => acc + u.age)
```

### Other sources

A **step closure** or an **`Iterable`** reaches the combinators through
`Iter.collect`, which drains it into a table:

```saule
Iter.map(Iter.collect(step), f)          -- a bare step closure
Iter.map(Iter.collect(list.iter()), f)   -- anything Iterable
```

That call is not overhead the design added: an eager `map` has to drain the
source before it can run, so `collect` only makes visible a cost that was
always there. It is a separate call rather than an overload because
`Iterable<V>` cannot be written in a native signature — but `iter()` is
declared by your own class, so `Iter.collect(list.iter())` is checked end to
end. A closure that never returns `nil` never terminates, exactly as the
hand-written `for` loop would not.

### Core

| Signature | Description |
| --- | --- |
| `Iter.collect<V>(step: fn() -> V?) -> table<V>` | Drain a step closure until it answers `nil`. |
| `Iter.map<V, U>(t: table<V>, f: fn(V) -> U) -> table<U>` | Apply `f` to every element. |
| `Iter.filter<V>(t: table<V>, pred: fn(V) -> boolean) -> table<V>` | Keep the elements `pred` accepts. |
| `Iter.reduce<V, A>(t: table<V>, init: A, f: fn(A, V) -> A) -> A` | Fold left from `init`. |
| `Iter.forEach<V>(t: table<V>, f: fn(V) -> nil) -> nil` | Run `f` for its effect on every element. |

### Search

| Signature | Description |
| --- | --- |
| `Iter.find<V>(t: table<V>, pred: fn(V) -> boolean) -> V?` | First element `pred` accepts, or `nil`. |
| `Iter.findIndex<V>(t: table<V>, pred: fn(V) -> boolean) -> integer?` | Its 1-based position. Named apart from `Table.indexOf`, which searches for a *value* rather than with a predicate. |
| `Iter.any<V>(t: table<V>, pred: fn(V) -> boolean) -> boolean` | Does any element match? `false` for an empty table. |
| `Iter.all<V>(t: table<V>, pred: fn(V) -> boolean) -> boolean` | Do all of them? `true` for an empty table. |
| `Iter.count<V>(t: table<V>, pred: fn(V) -> boolean) -> integer` | How many match. |

### Slicing

| Signature | Description |
| --- | --- |
| `Iter.take<V>(t: table<V>, n: integer) -> table<V>` | The first `n`, or all of them if there are fewer. |
| `Iter.skip<V>(t: table<V>, n: integer) -> table<V>` | Everything after the first `n`. |
| `Iter.first<V>(t: table<V>) -> V?` | First element, `nil` when empty. |
| `Iter.last<V>(t: table<V>) -> V?` | Last element, `nil` when empty. |
| `Iter.chunk<V>(t: table<V>, size: integer) -> table<table<V>>` | Fixed-size groups; the last is short when the length doesn't divide evenly. |

### Shaping

| Signature | Description |
| --- | --- |
| `Iter.zipWith<V, U, A>(a: table<V>, b: table<U>, f: fn(V, U) -> A) -> table<A>` | Pair the two up and combine, stopping at the shorter. |
| `Iter.flatten<V>(t: table<table<V>>) -> table<V>` | Concatenate the inner tables. One level only — nest the call to go deeper. |
| `Iter.reverse<V>(t: table<V>) -> table<V>` | A new reversed table. `Table.reverse` reverses in place instead. |
| `Iter.unique<V>(t: table<V>) -> table<V>` | Drop later duplicates, keeping input order. Compares with `==`, so `OpEq` applies. |
| `Iter.sortBy<V, K>(t: table<V>, key: fn(V) -> K) -> table<V>` | New table sorted ascending by the extracted key, using `<` (so `OpCompare` applies). Stable. For a bespoke ordering use `Table.sort`, which takes the comparator directly. |
| `Iter.groupBy<V, K>(t: table<V>, key: fn(V) -> K) -> table<K, table<V>>` | Bucket the elements by key. The key must be a `string`, `integer` or `boolean` — the types a table can be keyed by. |

There is no `Iter.zip` or `Iter.enumerate`. Both would have to return pairs,
and a pair has no representation here: a two-element table holding an
`integer` and a `V` types as `table<any>`, which loses both. `zipWith`
combines at the point the two elements meet instead, and a `for i, v in t do`
loop already gives you indices alongside values.

```saule
local scores: table<integer> = {84, 17, 96, 42}

println(Iter.count(scores, s => s >= 50))                   -- 2
println(Iter.find(scores, s => s > 90) ?? 0)                -- 96
println(String.join(",", Iter.sortBy(scores, s => s)))      -- 17,42,84,96
println(#Iter.chunk(scores, 3))                             -- 2
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
| `Io.lines(path: string?) -> fn() -> string?` | Line-iterator step closure. With no path, reads stdin. |
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
| `f.lines() -> fn() -> string?` | Per-line step closure. |
| `f.seek(whence: IoSeek?, offset: integer?) -> integer?` | Defaults `(Cur, 0)`. Returns the new position, or `nil` when the handle can't seek (a pipe, a closed file). |
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
| `Os.timeMillis() -> integer` | Unix epoch in milliseconds — the resolution you need to time a request, a frame, or a test. |
| `Os.clock() -> float` | Seconds from a **monotonic** clock. Measures elapsed wall-clock time and never goes backwards, but its origin is arbitrary, so only differences mean anything. Note this is *not* Lua's `os.clock`, which reports CPU time. |
| `Os.difftime(t2: integer, t1: integer) -> integer` | `t2 - t1`. |
| `Os.date(format: string?, time: integer?) -> string` | `strftime`-style. Default format `"%c"`, default time = now. |
| `Os.parsedate(text: string, format: string?) -> integer?` | The inverse of `Os.date`: parse `text` into a Unix epoch, or `nil` when it doesn't match. `format` defaults to `"%Y-%m-%d"`. |
| `Os.sleep(seconds: float) -> nil` | Fractions allowed for sub-second sleeps. |

Pick by the question you're asking: `Os.time` / `Os.timeMillis` for *when*
something happened (recordable, comparable across runs), `Os.clock` for *how
long* something took.

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
| `Os.list(path: string) -> table<string>` | Children of a directory (names only). **Throws** when the path can't be read, unlike its neighbours. |
| `Os.mkdir(path: string, recursive: boolean?) -> boolean` | Create a directory; `recursive` defaults to `false`. |
| `Os.remove(path: string) -> boolean` | Remove a file or empty directory. |
| `Os.rename(from: string, to: string) -> boolean` | Move/rename. |
| `Os.tmpname() -> string` | Path to a fresh temp file under the OS temp dir. |
| `Os.fsInfo(path: string?) -> FsInfo?` | Metadata for one path, or `nil` when it doesn't exist. |

### `FsInfo` fields

| Field | Type | Description |
| --- | --- | --- |
| `path` | `string` | The path this describes. |
| `kind` | `FsKind` | `File`, `Dir`, `Symlink` or `Other` (`.value` is `"file"` / `"dir"` / `"symlink"` / `"other"`). |
| `size` | `integer` | Size in bytes. |
| `modifiedAt` | `integer` | Last-modified time as a Unix epoch, for `Os.date`. |
| `readOnly` | `boolean` | Whether the permissions forbid writing. |

### Process

| Signature | Description |
| --- | --- |
| `Os.pid() -> integer` | This process's PID. |
| `Os.args() -> table<string>` | CLI args passed to the script. |
| `Os.execute(cmd: string) -> integer` | Run via the shell. Output goes to this process's own stdout/stderr; only the exit code comes back. |
| `Os.capture(cmd: string) -> (integer, string, string)` | Run via the shell and capture it: `(code, stdout, stderr)`. Nothing reaches the terminal. `-1` with empty output when the command can't be spawned. |
| `Os.exit(code: integer?) -> nil` | Terminate the process. |
| `Os.platform() -> OsPlatform` | Current OS as an enum (`.value` is `"linux"` / `"macos"` / `"windows"` / `"other"`). |

### `OsPlatform` variants

`Linux` (`"linux"`), `Macos` (`"macos"`), `Windows` (`"windows"`), `Other` (`"other"`).

```saule
local started: float = Os.clock()
Os.sleep(0.05)
printf("elapsed: %.3fs\n", Os.clock() - started)

local code, out, err = Os.capture("git rev-parse --short HEAD")
if code == 0 then
    println("at " .. String.trim(out))
end

if Os.platform().value == "linux" then
    println("home: " .. (Os.getenv("HOME") ?? "?"))
end
```

### Failure conventions

The filesystem calls disagree about how they report trouble, and it is worth
knowing which is which before you write the error handling:

- `Os.remove` / `Os.rename` / `Os.chdir` / `Os.mkdir` return `boolean`.
- `Os.fsInfo` and `Os.getenv` return `T?`.
- `Os.list` **throws**.

None of the first two carry the operating system's reason, so a failed call
can't tell "not found" from "permission denied". That is a known rough edge,
not a design.

---

## `Saule`

The version of the toolchain running your code. Distinct from `Project.version`,
which is the version of the code being run.

Saule versions are `<two-digit year>.<build number>` — `26.7` is the seventh
release cut in 2026. There is no patch component; a fix is simply the next
build number. Build numbers restart each year, and comparisons still work
because the year leads: `27.1` is newer than `26.412`.

### Constants

| Name | Type | Description |
| --- | --- | --- |
| `Saule.version` | `string` | `"26.7"` — the version as a version. Compare against this. |
| `Saule.full` | `string` | `"26.7"`, or `"26.8-dev+1a2b3c4"` for a development build. Display only — never parse it. |
| `Saule.year` | `integer` | `26`. |
| `Saule.build` | `integer` | `7`. Counts from 1 within the year; `0` means the version could not be determined. |
| `Saule.isDev` | `boolean` | `false` only when built from a clean release tag. |
| `Saule.commit` | `string` | Short commit hash, or `""` when it was built without git available. |

### Functions

| Signature | Description |
| --- | --- |
| `Saule.atLeast(version: string) -> boolean` | Is this toolchain `version` or newer? Compares dotted numeric components, so `"26.7"` satisfies `"26"` and `"26.7"` but not `"26.8"`. |

```saule
if Saule.atLeast("26.4") then
    println("running on " .. Saule.version)
end
```

`atLeast` is the runtime counterpart to `min_saule_version` in `saule.config`.
The two use the same comparison, but they answer different questions:
`min_saule_version` refuses to run the project at all on an old toolchain,
whereas `atLeast` lets code use a newer facility when it is available and fall
back when it isn't. Reach for `min_saule_version` when your project simply
cannot work without a version, and `atLeast` when it can degrade.

A development build reports the version it is *heading toward*, not the last
one released — so `26.8-dev` satisfies `atLeast("26.8")`. That is deliberate:
it lets you write and test code against a feature before its release exists.

---

## Conventions

A handful of patterns repeat across the stdlib:

- **1-based indexing everywhere.** `String.sub`, `String.iter`, `Table.*`,
  array literals — all 1-based. Negative indices count from the end where
  it makes sense.
- **`?` means "may not happen".** Lookups that can miss (`String.find`,
  `Io.open`, `Os.getenv`) return nullable types so the typechecker forces
  you to handle the absence with `?.`, `??`, `!`, or a `match`.
- **Multi-return is a tuple.** Functions like `String.find` and `Os.capture`
  return `(integer?, integer?)` / `(integer, string, string)`. Destructure
  with `local s, e = ...` or pattern-match.
- **Conversion is a cast, not a call.** `as` converts between `integer` and
  `float`, renders either (and `boolean`) as a `string`, and parses a
  `string` back — and on an `any` it is the checked type test instead. It
  refuses any pair with no obvious answer rather than inventing one.
- **Numeric flavour never mixes.** `integer` and `float` are separate types
  and nothing promotes between them silently — not the operators, and not the
  stdlib. `Math.*` is generic over the flavour (`<N>`), so a call takes either
  and answers in kind, but one call may not take both.
- **No exceptions for "expected" failures.** Filesystem helpers return
  `boolean` rather than throwing. Use `throw` / `try` / `catch` for genuinely
  exceptional cases — see [README §Error Handling](./README.md#error-handling).
  The exception is `Os.list`, which throws; see
  [Failure conventions](#failure-conventions).
- **Mutate with `Table`, transform with `Iter`.** `Table.*` writes to the
  table it is given or answers a question about it; `Iter.*` builds a new
  sequence and never writes. Where both could claim a name they are named
  apart — `Table.reverse` / `Iter.reverse`, `Table.indexOf` /
  `Iter.findIndex`.

Anything not in this reference is not in the toolchain. Libraries — JSON among
them — ship as ordinary Saule packages: a git repo with a `saule.config`,
added to `dependencies:`. See
[README §Importing from a Dependency](./README.md#importing-from-a-dependency).

