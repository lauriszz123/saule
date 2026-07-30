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
