---
title: "Os"
description: "Process, environment, time, and filesystem primitives. Filesystem calls return a boolean on success/failure (no detailed error type yet — wrap in…"
sidebar:
  order: 6
---

<!-- Generated from DOCS.md by `npm run sync-docs`. Edit that file, not this one. -->

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
| `Os.spawn(program: string, args: table<string>?, cwd: string?, env: table<string, string>?) -> Process?` | Start `program` and return a handle **while it runs** — see [`Process` handles](/saule/stdlib/os/#process-handles). No shell: `args` is passed as a vector. `cwd` sets the working directory; `env` adds to or overrides the inherited environment. `nil` when it can't be started. |
| `Os.exit(code: integer?) -> nil` | Terminate the process. |
| `Os.platform() -> OsPlatform` | Current OS as an enum (`.value` is `"linux"` / `"macos"` / `"windows"` / `"other"`). |

### `OsPlatform` variants

`Linux` (`"linux"`), `Macos` (`"macos"`), `Windows` (`"windows"`), `Other` (`"other"`).

### `Process` handles

`Os.execute` and `Os.capture` both block until the child is finished, so
neither can be used to *talk to* one. `Os.spawn` is for a child with a
conversation in it — a language server, a formatter fed on stdin, an editor
embedded in a window: it returns while the child runs, and the program
writes, reads what came back, and carries on in between.

| Signature | Description |
| --- | --- |
| `p.read(max: integer?) -> bytes` | Whatever has arrived on stdout, up to `max` (default 64 KiB). **Never blocks.** The empty buffer means "nothing yet", which is not the same as end of output — see `atEof`. |
| `p.readErr(max: integer?) -> bytes` | The same for stderr, which is a separate stream. |
| `p.write(data: bytes) -> integer` | Write to the child's stdin; returns the byte count. An error once stdin is closed. |
| `p.closeStdin() -> nil` | Drop stdin so the child sees end of input. Idempotent. Needed by anything that reads until EOF. |
| `p.poll() -> integer?` | The exit code if it has finished, `nil` while it runs. Remembered, so asking twice is fine. |
| `p.wait() -> integer` | Block until it exits, and return the code. Closes stdin first — a child waiting for input and a parent waiting for the child is a deadlock, and this is the end that can tell. |
| `p.kill() -> boolean` | End it. `false` if it had already exited, which is not an error. |
| `p.running() -> boolean` | Is it still going? Asks the OS, so it is accurate without a prior `poll`. |
| `p.pid() -> integer` | The child's PID. |
| `p.available() -> integer` | Bytes waiting on stdout, without taking them — how far behind a reader is. |
| `p.atEof() -> boolean` | Has stdout ended **and** been drained? Both halves matter: a child can exit with output still buffered, and reporting eof then would lose it. |

Three things worth knowing:

**Reads are typed `bytes`, not `string`.** A child's output is not promised to
be text — it can be an image, a compressed stream, a binary wire protocol —
and a `string` is always valid UTF-8. Use `Bytes.toString` where you expect
text; it returns `string?` precisely because this is where that can fail.

**No shell.** `args` is a vector, so nothing re-parses quoting and a path with
a space in it stays one argument. `Os.execute("ls $HOME")` expands that
variable; `Os.spawn("ls", {"$HOME"})` passes it through literally. Ask for a
shell by name when you want one.

**`cwd` and `env` are how you place the child.** `cwd` is its working
directory, which is what you want for a tool that reads a config from the
directory it starts in. `env` is an **overlay** on the environment the child
inherits, not a replacement — a child handed an environment with no `PATH`
usually cannot run at all, so there is deliberately no way to start it from
an empty one. A `cwd` that does not exist is a spawn failure like any other,
so the result is `nil`.

```saule
-- A language server, rooted in the project and told where to look.
local lsp: Process = Os.spawn(
    "saule-lsp", {"--stdio"},
    projectRoot,
    {SAULE_LOG: "debug"}
)!
```

**The child outlives the handle.** Dropping the last reference does not kill
it. Call `kill` or `wait`.

**Each stream buffers at most 8 MiB of unread output.** At the limit the
reader stops taking from the pipe, so the operating system's pipe buffer
fills and the child blocks on its next write — nothing is ever dropped, which
for a binary protocol would be unrecoverable. An interactive child deals in
kilobytes per exchange, so a program that reads once a frame sits near zero;
hitting the limit means nobody is reading, and `p.available()` is how you see
it coming. `wait` lifts the limit for the rest of the child's life, since
"run to completion" and "stop reading you" cannot both hold.

```saule
-- Drive a child that answers on stdout, without ever blocking.
local p: Process = Os.spawn("nvim", {"--embed", "--headless"})!

p.write(request)                -- a `bytes` you built

local deadline: float = Os.clock() + 5.0
local reply: bytes = Bytes.empty()
while #reply == 0 and Os.clock() < deadline do
    reply = p.read()
    if #reply == 0 then
        Os.sleep(0.01)          -- or: hand control back to a frame loop
    end
end

println(Bytes.toHex(reply))
p.kill()
```

Because `read` returns immediately, that loop is equally at home as one call
per frame in a UI: read what arrived, draw, come back next frame.

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
