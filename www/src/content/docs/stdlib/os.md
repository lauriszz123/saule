---
title: "Os"
description: "Time, environment, filesystem and processes. This is the module that touches the world outside the program, so it is also where failure is most…"
sidebar:
  order: 9
---

<!-- Generated from DOCS.md by `npm run sync-docs`. Edit that file, not this one. -->

Time, environment, filesystem and processes. This is the module that touches
the world outside the program, so it is also where failure is most ordinary —
see [Failure conventions](/saule/stdlib/os/#failure-conventions) for which call reports it
which way.

### Constants

| Name | Type | Value |
| --- | --- | --- |
| `Os.sep` | `string` | The path separator: `"/"`, or `"\\"` on Windows |
| `Os.lineSep` | `string` | The line ending: `"\n"`, or `"\r\n"` on Windows |

```saule
-- Joining a path without hard-coding a separator.
local parts: table<string> = {"src", "main.sau"}
println(String.join(Os.sep, parts))      -- src/main.sau on Unix

println(#Os.lineSep >= 1)                -- true
```

### Time

<div class="api-list">
<details class="api-entry" id="os-time">
<summary><code class="api-sig"><span class="sig-fn">Os.time</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">integer</span></code></summary>


The current Unix epoch in **seconds**. Comparable across runs and across
machines, which is what makes it the right thing to record.

```saule
local now: integer = Os.time()
println(now > 1700000000)       -- true, well past 2023

-- A timestamp to store or print.
println(Os.date("%Y-%m-%d", now) != "")   -- true
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="os-timemillis">
<summary><code class="api-sig"><span class="sig-fn">Os.timeMillis</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">integer</span></code></summary>


The same instant in **milliseconds** — the resolution needed to time a
request, a frame or a test.

```saule
local started: integer = Os.timeMillis()
Os.sleep(0.05)
local elapsed: integer = Os.timeMillis() - started

println(elapsed >= 40)          -- true, about 50ms
println(elapsed < 5000)         -- true
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="os-clock">
<summary><code class="api-sig"><span class="sig-fn">Os.clock</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">float</span></code></summary>


Seconds from a **monotonic** clock: it never goes backwards, even if the
system clock is adjusted, but its origin is arbitrary — only *differences*
mean anything.

This is not Lua's `os.clock`, which reports CPU time. Use `Os.time` for
*when* something happened and `Os.clock` for *how long* it took.

```saule
local started: float = Os.clock()
Os.sleep(0.05)
local took: float = Os.clock() - started

println(took >= 0.04)           -- true
println(took < 5.0)             -- true
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="os-difftime">
<summary><code class="api-sig"><span class="sig-fn">Os.difftime</span><span class="sig-punct">(</span><span class="sig-param">t2</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">,</span> <span class="sig-param">t1</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">integer</span></code></summary>


`t2 - t1`. Present for familiarity with Lua; plain subtraction does the same
thing here, because both arguments are already integers.

```saule
local earlier: integer = Os.time()
Os.sleep(0.05)
local later: integer = Os.time()

println(Os.difftime(later, earlier) >= 0)      -- true
println(Os.difftime(100, 40))                  -- 60
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="os-date">
<summary><code class="api-sig"><span class="sig-fn">Os.date</span><span class="sig-punct">(</span><span class="sig-param">format</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">?</span><span class="sig-punct">,</span> <span class="sig-param">time</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">?</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">string</span></code></summary>


Formats a Unix epoch using `strftime`-style specifiers. `format` defaults to
`"%c"` and `time` to now.

These are the specifiers that are implemented:

| Specifier | Gives | Example |
| --- | --- | --- |
| `%Y` | Four-digit year | `2021` |
| `%y` | Two-digit year | `21` |
| `%m` | Month, zero-padded | `01` |
| `%d` | Day of month, zero-padded | `01` |
| `%j` | Day of year, zero-padded | `001` |
| `%H` `%M` `%S` | Hours, minutes, seconds | `00` |
| `%c` | Date and time | `2021-01-01 00:00:00` |
| `%x` | Date only | `2021-01-01` |
| `%X` | Time only | `00:00:00` |

Anything else is **passed through literally** rather than reported — the
name-based specifiers (`%A`, `%B`, `%a`, `%b`, `%p`) are not implemented, so
`Os.date("%B")` answers the string `"%B"`.

```saule
-- A fixed instant, so the output is the same every run:
-- 2021-01-01 00:00:00 UTC.
local stamp: integer = 1609459200

println(Os.date("%Y-%m-%d", stamp))           -- 2021-01-01
println(Os.date("%Y-%m-%dT%H:%M:%S", stamp))  -- 2021-01-01T00:00:00
println(Os.date("%x", stamp))                 -- 2021-01-01
println(Os.date("%B", stamp))                 -- %B, not implemented

-- No arguments at all is "now, in the system's preferred format".
println(#Os.date() > 0)                       -- true
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="os-parsedate">
<summary><code class="api-sig"><span class="sig-fn">Os.parsedate</span><span class="sig-punct">(</span><span class="sig-param">text</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">,</span> <span class="sig-param">format</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">?</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">integer</span><span class="sig-punct">?</span></code></summary>


The inverse of `Os.date`: reads `text` and answers the Unix epoch, or `nil`
when it does not match. `format` defaults to `"%Y-%m-%d"`.

```saule
println(Os.parsedate("2021-01-01") ?? 0)                    -- 1609459200
println(Os.parsedate("not a date") == nil)                  -- true

-- A format other than the default.
local at: integer? = Os.parsedate("01/01/2021", "%d/%m/%Y")
println(at ?? 0)                                            -- 1609459200

-- Round trip.
local stamp: integer = 1609459200
println(Os.parsedate(Os.date("%Y-%m-%d", stamp)) == stamp)  -- true
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="os-sleep">
<summary><code class="api-sig"><span class="sig-fn">Os.sleep</span><span class="sig-punct">(</span><span class="sig-param">seconds</span><span class="sig-punct">:</span> <span class="sig-type">float</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-kw">nil</span></code></summary>


Pauses the whole program. Fractions are allowed, so sub-second waits are
ordinary.

```saule
local started: float = Os.clock()
Os.sleep(0.1)
println(Os.clock() - started >= 0.09)    -- true

-- A retry loop that backs off.
for attempt = 1, 3 do
    Os.sleep(0.01 * (attempt as float))
end
println("done")                          -- done
```


</details>
</div>

### Environment

<div class="api-list">
<details class="api-entry" id="os-getenv">
<summary><code class="api-sig"><span class="sig-fn">Os.getenv</span><span class="sig-punct">(</span><span class="sig-param">name</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">string</span><span class="sig-punct">?</span></code></summary>


The value of an environment variable, or `nil` when it is not set.

```saule
Os.setenv("SAULE_DOC_EXAMPLE", "yes")
println(Os.getenv("SAULE_DOC_EXAMPLE") ?? "")      -- yes

-- Unset is `nil`, which `??` turns into a default.
println(Os.getenv("SAULE_DEFINITELY_UNSET") ?? "fallback")   -- fallback

-- The shape configuration usually wants.
local level: string = Os.getenv("LOG_LEVEL") ?? "info"
println(level != "")                               -- true
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="os-setenv">
<summary><code class="api-sig"><span class="sig-fn">Os.setenv</span><span class="sig-punct">(</span><span class="sig-param">name</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">,</span> <span class="sig-param">value</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-kw">nil</span></code></summary>


Sets a variable **for this process** and anything it spawns afterwards. It
does not reach the shell that started the program.

```saule
Os.setenv("SAULE_GREETING", "hello")
println(Os.getenv("SAULE_GREETING") ?? "")         -- hello

-- Which a child process then inherits.
local code: integer, out: string, err: string = Os.capture("echo $SAULE_GREETING")
println(String.trim(out))                          -- hello
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="os-cwd">
<summary><code class="api-sig"><span class="sig-fn">Os.cwd</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">string</span></code></summary>


The absolute path of the current working directory.

```saule
local here: string = Os.cwd()
println(String.len(here) > 0)      -- true
println(String.starts(here, Os.sep) or String.contains(here, ":"))   -- true

-- Resolving a relative path against it.
println(Os.exists(here))           -- true
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="os-chdir">
<summary><code class="api-sig"><span class="sig-fn">Os.chdir</span><span class="sig-punct">(</span><span class="sig-param">path</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">boolean</span></code></summary>


Changes the working directory, answering `false` when it cannot — the path
does not exist, or is not a directory.

```saule
local original: string = Os.cwd()

println(Os.chdir("/"))                  -- true
println(Os.cwd() != original)           -- true

println(Os.chdir("/no/such/dir"))       -- false, and nothing moved

Os.chdir(original)
println(Os.cwd() == original)           -- true
```


</details>
</div>

### Filesystem

<div class="api-list">
<details class="api-entry" id="os-exists">
<summary><code class="api-sig"><span class="sig-fn">Os.exists</span><span class="sig-punct">(</span><span class="sig-param">path</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">boolean</span></code></summary>


Whether anything exists at `path` — a file, a directory or a symlink.

```saule
local path: string = Os.tmpname()
println(Os.exists(path))             -- false, not created yet

Io.open(path, IoMode.Write)!.close()
println(Os.exists(path))             -- true

Os.remove(path)
println(Os.exists(path))             -- false again
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="os-list">
<summary><code class="api-sig"><span class="sig-fn">Os.list</span><span class="sig-punct">(</span><span class="sig-param">path</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">string</span><span class="sig-punct">&gt;</span></code></summary>


The names of a directory's children — names only, not full paths.

**It is the one call here that fails hard.** When the path cannot be read it
raises a runtime error that ends the program; `try` / `catch` does not
intercept it, because this is not a `throw` of a value. Its neighbours answer
`false` or `nil` instead, so check with `Os.exists` or `Os.fsInfo` first when
the path might not be there. That inconsistency is a known rough edge — see
[Failure conventions](/saule/stdlib/os/#failure-conventions).

```saule
local dir: string = Os.tmpname()
Os.mkdir(dir)
Io.open(dir .. Os.sep .. "a.txt", IoMode.Write)!.close()
Io.open(dir .. Os.sep .. "b.txt", IoMode.Write)!.close()

local names: table<string> = Os.list(dir)
Table.sort(names, (a, b) => a < b)
println(Table.concat(names, ","))     -- a.txt,b.txt

-- Names, so join them back onto the directory to use them.
for name in names do
    println(Os.exists(dir .. Os.sep .. name))   -- true, twice
end

-- Guard rather than catch: an unreadable path ends the program.
if Os.exists("/no/such/dir") then
    println(#Os.list("/no/such/dir"))
else
    println("not a readable directory")   -- not a readable directory
end

Os.remove(dir .. Os.sep .. "a.txt")
Os.remove(dir .. Os.sep .. "b.txt")
Os.remove(dir)
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="os-mkdir">
<summary><code class="api-sig"><span class="sig-fn">Os.mkdir</span><span class="sig-punct">(</span><span class="sig-param">path</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">,</span> <span class="sig-param">recursive</span><span class="sig-punct">:</span> <span class="sig-type">boolean</span><span class="sig-punct">?</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">boolean</span></code></summary>


Creates a directory, answering `false` when it cannot. With `recursive` set,
it creates the missing parents too; without it, a missing parent is a
failure.

```saule
local base: string = Os.tmpname()

println(Os.mkdir(base))                            -- true
println(Os.mkdir(base))                            -- false, already there

-- A nested path needs `recursive`.
local nested: string = base .. Os.sep .. "a" .. Os.sep .. "b"
println(Os.mkdir(nested))                          -- false, parent missing
println(Os.mkdir(nested, true))                    -- true
println(Os.exists(nested))                         -- true

Os.remove(nested)
Os.remove(base .. Os.sep .. "a")
Os.remove(base)
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="os-remove">
<summary><code class="api-sig"><span class="sig-fn">Os.remove</span><span class="sig-punct">(</span><span class="sig-param">path</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">boolean</span></code></summary>


Removes a file, or an **empty** directory. `false` when it cannot — the path
is missing, or the directory still has something in it.

```saule
local path: string = Os.tmpname()
Io.open(path, IoMode.Write)!.close()

println(Os.remove(path))              -- true
println(Os.remove(path))              -- false, already gone

-- A directory has to be empty first.
local dir: string = Os.tmpname()
Os.mkdir(dir)
local inside: string = dir .. Os.sep .. "f.txt"
Io.open(inside, IoMode.Write)!.close()

println(Os.remove(dir))               -- false, not empty
println(Os.remove(inside))            -- true
println(Os.remove(dir))               -- true, now it is
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="os-rename">
<summary><code class="api-sig"><span class="sig-fn">Os.rename</span><span class="sig-punct">(</span><span class="sig-param">from</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">,</span> <span class="sig-param">to</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">boolean</span></code></summary>


Moves or renames, answering `false` when it cannot. Within one filesystem
this is atomic, which is what makes write-then-rename the safe way to replace
a file.

```saule
local a: string = Os.tmpname()
local b: string = a .. ".renamed"

local f: File = Io.open(a, IoMode.Write)!
f.write("contents")
f.close()

println(Os.rename(a, b))                  -- true
println(Os.exists(a))                     -- false
println(Io.open(b, IoMode.Read)!.read("a") ?? "")   -- contents

println(Os.rename("/no/such/file", b))    -- false

Os.remove(b)
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="os-tmpname">
<summary><code class="api-sig"><span class="sig-fn">Os.tmpname</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">string</span></code></summary>


A path under the system temp directory that nothing is using yet. It is a
**name**, not a file: nothing is created until you create it.

```saule
local path: string = Os.tmpname()
println(Os.exists(path))          -- false, just a name

local f: File = Io.open(path, IoMode.Write)!
f.write("x")
f.close()
println(Os.exists(path))          -- true

Os.remove(path)

-- Two calls never collide.
println(Os.tmpname() != Os.tmpname())   -- true
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="os-fsinfo">
<summary><code class="api-sig"><span class="sig-fn">Os.fsInfo</span><span class="sig-punct">(</span><span class="sig-param">path</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">?</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">FsInfo</span><span class="sig-punct">?</span></code></summary>


Metadata for one path, or `nil` when nothing is there. `path` defaults to the
current directory.

The result is an `FsInfo` with these fields:

| Field | Type | What it holds |
| --- | --- | --- |
| `path` | `string` | The path this describes |
| `kind` | `FsKind` | `File`, `Dir`, `Symlink` or `Other` |
| `size` | `integer` | Size in bytes |
| `modifiedAt` | `integer` | Last-modified time, as a Unix epoch for `Os.date` |
| `readOnly` | `boolean` | Whether the permissions forbid writing |

```saule
local path: string = Os.tmpname()
local f: File = Io.open(path, IoMode.Write)!
f.write("12345")
f.close()

local info: FsInfo = Os.fsInfo(path)!
println(info.size)                      -- 5
println(info.kind.value)                -- file
println(info.readOnly)                  -- false
println(info.modifiedAt > 0)            -- true
println(info.path == path)              -- true

-- A directory reports its own kind.
println(Os.fsInfo(Os.cwd())!.kind.value)   -- dir

-- Nothing there is `nil`, not an error.
println(Os.fsInfo("/no/such/path") == nil) -- true

Os.remove(path)
```


</details>
</div>

### `FsKind` variants

`File` (`"file"`), `Dir` (`"dir"`), `Symlink` (`"symlink"`) and `Other`
(`"other"`). Match on the variant rather than comparing `.value` when you
want the compiler to check you have covered every case.

```saule
local info: FsInfo = Os.fsInfo(Os.cwd())!

match info.kind
case FsKind.Dir then
    println("a directory")      -- a directory
case FsKind.File then
    println("a file")
case _ then
    println("something else")
end
```

### Process

<div class="api-list">
<details class="api-entry" id="os-pid">
<summary><code class="api-sig"><span class="sig-fn">Os.pid</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">integer</span></code></summary>


This process's own process id.

```saule
println(Os.pid() > 0)            -- true

-- Stable within a run.
println(Os.pid() == Os.pid())    -- true
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="os-args">
<summary><code class="api-sig"><span class="sig-fn">Os.args</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">string</span><span class="sig-punct">&gt;</span></code></summary>


The command-line arguments passed to the script, without the interpreter or
the script path.

```saule
local args: table<string> = Os.args()
println(#args >= 0)              -- true

-- The shape a CLI usually wants.
local first: string = args[1] ?? "<none>"
println(first != "")             -- true
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="os-execute">
<summary><code class="api-sig"><span class="sig-fn">Os.execute</span><span class="sig-punct">(</span><span class="sig-param">cmd</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">integer</span></code></summary>


Runs `cmd` **through the shell** and answers its exit code. The child's
output goes to this process's own stdout and stderr — nothing is captured.

`-1` when the command could not be run at all.

```saule
println(Os.execute("true"))             -- 0
println(Os.execute("false") != 0)       -- true

-- The shell is real, so redirection and variables work.
println(Os.execute("test -d /") == 0)   -- true
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="os-capture">
<summary><code class="api-sig"><span class="sig-fn">Os.capture</span><span class="sig-punct">(</span><span class="sig-param">cmd</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-punct">(</span><span class="sig-type">integer</span><span class="sig-punct">,</span> <span class="sig-type">string</span><span class="sig-punct">,</span> <span class="sig-type">string</span><span class="sig-punct">)</span></code></summary>


Runs `cmd` through the shell and captures it: the exit code, everything it
wrote to stdout, and everything it wrote to stderr. Nothing reaches the
terminal.

`-1` with two empty strings when the command could not be run.

```saule
local code: integer, out: string, err: string = Os.capture("echo hello")

println(code)                           -- 0
println(String.trim(out))               -- hello
println(err == "")                      -- true

-- stderr comes back separately from stdout.
local c2: integer, o2: string, e2: string = Os.capture("echo oops 1>&2")
println(o2 == "")                       -- true
println(String.trim(e2))                -- oops

-- A failing command reports its code rather than throwing.
local c3: integer, o3: string, e3: string = Os.capture("exit 3")
println(c3)                             -- 3
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="os-spawn">
<summary><code class="api-sig"><span class="sig-fn">Os.spawn</span><span class="sig-punct">(</span><span class="sig-param">program</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">,</span> <span class="sig-param">args</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">string</span><span class="sig-punct">&gt;</span><span class="sig-punct">?</span><span class="sig-punct">,</span> <span class="sig-param">cwd</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">?</span><span class="sig-punct">,</span> <span class="sig-param">env</span><span class="sig-punct">:</span> <span class="sig-type">table</span><span class="sig-punct">&lt;</span><span class="sig-type">string</span><span class="sig-punct">,</span> <span class="sig-type">string</span><span class="sig-punct">&gt;</span><span class="sig-punct">?</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">Process</span><span class="sig-punct">?</span></code></summary>


Starts `program` and answers a handle **while it runs**, so the program can
write to it, read what came back, and carry on in between. `nil` when it
cannot be started.

There is **no shell**: `args` is passed as a vector, so nothing re-parses
quoting and a path with a space in it stays one argument. `cwd` sets the
child's working directory, and `env` *adds to* the environment it inherits
rather than replacing it.

See [`Process` handles](/saule/stdlib/os/#process-handles) for what the handle can do.

```saule
-- A child that finishes on its own.
local p: Process = Os.spawn("echo", {"hello"})!
p.wait()
println(String.trim(Bytes.toString(p.read()) ?? ""))   -- hello

-- No shell: the argument arrives exactly as written.
local literal: Process = Os.spawn("echo", {"$HOME not expanded"})!
literal.wait()
println(String.trim(Bytes.toString(literal.read()) ?? ""))  -- $HOME not expanded

-- `cwd` places the child.
local inDir: Process = Os.spawn("pwd", {}, "/")!
inDir.wait()
println(String.trim(Bytes.toString(inDir.read()) ?? ""))    -- /

-- A program that is not there is `nil`.
println(Os.spawn("saule-no-such-program") == nil)           -- true
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="os-exit">
<summary><code class="api-sig"><span class="sig-fn">Os.exit</span><span class="sig-punct">(</span><span class="sig-param">code</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">?</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-kw">nil</span></code></summary>


Ends the process immediately with `code`, which defaults to 0. Nothing after
it runs.

```saule
-- Guarded so this example still prints, rather than exiting the page.
local shouldStop: boolean = false
if shouldStop then
    Os.exit(1)
end
println("still running")       -- still running
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="os-platform">
<summary><code class="api-sig"><span class="sig-fn">Os.platform</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">OsPlatform</span></code></summary>


Which operating system this is, as an enum: `Linux`, `Macos`, `Windows` or
`Other`, whose `.value` is `"linux"`, `"macos"`, `"windows"` or `"other"`.

```saule
println(#Os.platform().value > 0)      -- true

-- Branching on it, checked for exhaustiveness.
match Os.platform()
case OsPlatform.Windows then
    println("backslashes")
case _ then
    println("slashes")                 -- slashes, on Unix
end

-- Which is also what `Os.sep` already answers.
println(Os.sep == "/" or Os.sep == "\\")   -- true
```


</details>
</div>

### `Process` handles

`Os.execute` and `Os.capture` both block until the child is finished, so
neither can be used to *talk to* one. `Os.spawn` is for a child with a
conversation in it — a language server, a formatter fed on stdin, an editor
embedded in a window: it returns while the child runs, and the program
writes, reads what came back, and carries on in between.

Two properties shape everything below. **Reads never block**: each captured
stream is drained on a thread of its own into a buffer, and `read` takes
whatever has arrived so far. And **reads are `bytes`, not `string`**: a
child's output is not promised to be text, and a `string` is always valid
UTF-8 — use `Bytes.toString` where you expect text, which returns `string?`
precisely because that is where it can fail.

```saule
local p: Process = Os.spawn("cat")!

p.write(Bytes.fromString("round trip\n"))
p.closeStdin()                -- `cat` reads until end of input
println(p.wait())             -- 0, its exit code

println(String.trim(Bytes.toString(p.read()) ?? ""))   -- round trip
```

<div class="api-list">
<details class="api-entry" id="p-read">
<summary><code class="api-sig"><span class="sig-fn">p.read</span><span class="sig-punct">(</span><span class="sig-param">max</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">?</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">bytes</span></code></summary>


Whatever has arrived on stdout, up to `max` bytes (64 KiB by default).
**Never blocks**: an empty buffer means "nothing yet", which is not the same
as "nothing more" — see [`p.atEof()`](/saule/stdlib/os/#p-ateof).

```saule
local p: Process = Os.spawn("echo", {"abcdefgh"})!
p.wait()

println(Bytes.toString(p.read(3)) ?? "")     -- abc, capped
println(#p.read(0))                          -- 0, reads nothing
println(String.trim(Bytes.toString(p.read()) ?? ""))   -- defgh, the rest
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="p-readerr">
<summary><code class="api-sig"><span class="sig-fn">p.readErr</span><span class="sig-punct">(</span><span class="sig-param">max</span><span class="sig-punct">:</span> <span class="sig-type">integer</span><span class="sig-punct">?</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">bytes</span></code></summary>


The same, for stderr. The two streams are separate, so output written to one
never appears in the other.

```saule
local p: Process = Os.spawn("sh", {"-c", "echo out; echo err 1>&2"})!
p.wait()

println(String.trim(Bytes.toString(p.read()) ?? ""))      -- out
println(String.trim(Bytes.toString(p.readErr()) ?? ""))   -- err
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="p-write">
<summary><code class="api-sig"><span class="sig-fn">p.write</span><span class="sig-punct">(</span><span class="sig-param">data</span><span class="sig-punct">:</span> <span class="sig-type">bytes</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">integer</span></code></summary>


Writes to the child's stdin and answers how many bytes went in. An error once
stdin has been closed.

```saule
local p: Process = Os.spawn("cat")!

println(p.write(Bytes.fromString("hello")))   -- 5, the byte count
p.closeStdin()
p.wait()
println(Bytes.toString(p.read()) ?? "")       -- hello
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="p-closestdin">
<summary><code class="api-sig"><span class="sig-fn">p.closeStdin</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-kw">nil</span></code></summary>


Drops the child's stdin so it sees end of input. Idempotent — closing twice
has expressed one intention.

Anything that reads until EOF needs this, or both ends wait forever.

```saule
local p: Process = Os.spawn("cat")!
p.write(Bytes.fromString("done\n"))

-- Without this, `cat` keeps waiting for more and `wait` never returns.
p.closeStdin()
println(p.wait())                             -- 0
println(String.trim(Bytes.toString(p.read()) ?? ""))   -- done
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="p-poll">
<summary><code class="api-sig"><span class="sig-fn">p.poll</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">integer</span><span class="sig-punct">?</span></code></summary>


The exit code if the child has finished, `nil` while it is still running.
Remembered, so asking twice is fine.

```saule
local p: Process = Os.spawn("sleep", {"30"})!
println(p.poll() == nil)        -- true, still going

p.kill()
p.wait()
println(p.poll() != nil)        -- true, and it stays answered
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="p-wait">
<summary><code class="api-sig"><span class="sig-fn">p.wait</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">integer</span></code></summary>


Blocks until the child exits **and its output has arrived**, then answers its
exit code. Closes stdin first — a child waiting for input and a parent
waiting for the child is a deadlock, and this is the end that can tell.

Both halves matter: the exit status alone does not mean the output has been
collected, so `wait` then `read` sees all of it rather than however much
happened to arrive first.

```saule
local p: Process = Os.spawn("echo", {"complete"})!
println(p.wait())                                       -- 0
println(String.trim(Bytes.toString(p.read()) ?? ""))    -- complete

-- A non-zero code comes back as itself.
local failed: Process = Os.spawn("sh", {"-c", "exit 3"})!
println(failed.wait())                                  -- 3
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="p-kill">
<summary><code class="api-sig"><span class="sig-fn">p.kill</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">boolean</span></code></summary>


Ends the child. `true` when the signal was delivered, `false` when it had
already exited — which is not an error.

```saule
local p: Process = Os.spawn("sleep", {"30"})!
println(p.running())            -- true
println(p.kill())               -- true

-- Signalled rather than exited, which `Os.execute` also reports as -1.
println(p.wait())               -- -1
println(p.running())            -- false
println(p.kill())               -- false, already gone
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="p-running">
<summary><code class="api-sig"><span class="sig-fn">p.running</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">boolean</span></code></summary>


Whether the child is still going. Asks the operating system rather than
trusting a flag, so it is accurate without a prior `poll`.

```saule
local p: Process = Os.spawn("sleep", {"30"})!
println(p.running())      -- true
p.kill()
p.wait()
println(p.running())      -- false
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="p-pid">
<summary><code class="api-sig"><span class="sig-fn">p.pid</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">integer</span></code></summary>


The child's process id.

```saule
local p: Process = Os.spawn("true")!
println(p.pid() > 0)            -- true
println(p.pid() != Os.pid())    -- true, a different process
p.wait()
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="p-available">
<summary><code class="api-sig"><span class="sig-fn">p.available</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">integer</span></code></summary>


How many bytes are waiting on stdout, without taking them — how far behind a
reader is.

Each stream buffers at most 8 MiB of unread output. At the limit the reader
stops taking from the pipe, so the operating system's own buffer fills and
the child blocks on its next write: nothing is ever dropped, which for a
binary protocol would be unrecoverable.

```saule
local p: Process = Os.spawn("echo", {"hello"})!
p.wait()

println(p.available())          -- 6, "hello" and a newline
p.read()
println(p.available())          -- 0, taken
```


</details>
</div>

<div class="api-list">
<details class="api-entry" id="p-ateof">
<summary><code class="api-sig"><span class="sig-fn">p.atEof</span><span class="sig-punct">(</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">boolean</span></code></summary>


Whether stdout has **ended and been drained**. Both halves matter: a child
can exit with output still sitting in the buffer, and reporting end of file
then would lose it.

This is what makes a non-blocking read loop terminate correctly.

```saule
local p: Process = Os.spawn("sh", {"-c", "for i in 1 2 3; do echo $i; done"})!

local text: string = ""
local deadline: float = Os.clock() + 20.0
while Os.clock() < deadline do
    text = text .. (Bytes.toString(p.read()) ?? "")
    -- Done only when it has exited *and* the buffer is drained; either
    -- alone would lose the tail.
    if p.poll() != nil and p.atEof() then
        break
    end
end

println(String.replace(String.trim(text), "\n", ","))   -- 1,2,3
println(p.atEof())                                      -- true
```

**The child outlives the handle.** Dropping the last reference does not kill
it — call `kill` or `wait`.


</details>
</div>

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
