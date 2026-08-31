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
