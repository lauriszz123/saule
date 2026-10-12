---
title: CLI
description: Every subcommand and flag the saule binary accepts.
---

```sh
saule [OPTIONS] [COMMAND]
```

Running bare `saule` prints help and exits successfully.

| Option | Description |
|---|---|
| `-v`, `--version` | Print the version and exit |

## `saule run`

Run a project or a single source file.

```sh
saule run [TARGET] [-- ARGS...]
```

| Argument | Description |
|---|---|
| `TARGET` | A project directory or a `.sau` file. Defaults to the current directory. |
| `ARGS` | Everything after `--`, forwarded verbatim to the script's `Os.args()` |

Whether `TARGET` is a directory is the **only** thing that picks the mode:

| Invocation | What runs |
|---|---|
| `saule run` | The project in the current directory |
| `saule run <dir>` | The project rooted at `<dir>` |
| `saule run <file.sau>` | That file, on its own |
| `saule run -- a b` | The current project, with `Os.args() == ["a", "b"]` |
| `saule run <file> -- a b` | That file, with `Os.args() == ["a", "b"]` |

**Project mode** requires the file named by `entry:` in `saule.config` to
declare a `Main` class with a `static fn main()`. Top-level statements in that
file execute first, then `Main.main()` is called.

**Single-file mode** executes the file top-to-bottom like a Lua script. No
`Main` class is required, and any surrounding `saule.config` is ignored. If the
script does happen to define `Main.main()`, it is invoked after the top-level
body finishes.

Script arguments go after `--` and are never interpreted by the CLI — which is
what lets a program take a filename or a flag of its own:

```sh
saule run -- input.bf          # project in the cwd, Os.args() = ["input.bf"]
saule run tool.sau -- -v file  # single file; script args may start with `-`
```

## `saule check`

Type-check a project or a source file without running it.

```sh
saule check [OPTIONS] [TARGET]
```

| Argument | Description |
|---|---|
| `TARGET` | A project directory or a `.sau` file. Defaults to the current directory. |

| Option | Description |
|---|---|
| `--json` | Write the diagnostics to stdout as JSON instead of rendering them |
| `--dump-type-coverage` | Report how much of each file the type checker proved a type for |

`TARGET` follows exactly the same rule as `run`: a directory (or no argument at
all) means project mode, a file means that file alone. Two differences from
`run` are deliberate:

- **Every diagnostic, not just the first.** `run` stops at the first because it
  is about to execute and the first error already settled that. `check` reports
  the whole list so it can be fixed in one pass.
- **Every file, not just what the entry point reaches.** A module nothing
  imports yet is precisely the one whose errors are worth hearing about before
  it is imported.

Libraries (`kind: "library"`) have no entry point and check normally. Exits with
status 1 when anything is reported, so it can gate CI.

### `--json`

For anything that has to *act* on the error list rather than read it: a build
script, an editor not using the language server, a CI job, or a harness running
the compiler over a great many programs. The rendered form is for a person, and
screen-scraping it fails quietly the moment the layout changes.

```sh
saule check src/main.sau --json
```

```json
{
  "saule_version": "26.6-dev+9ce2dbb",
  "commit": "9ce2dbb",
  "ok": false,
  "files": [
    {
      "path": "src/main.sau",
      "diagnostics": [
        {
          "severity": "error",
          "phase": "type",
          "message": "cannot assign value of type `any` to variable of type `integer`",
          "span": { "start": 226, "end": 227 },
          "help": "the value's type isn't compatible with the declared type"
        }
      ]
    }
  ],
  "summary": { "files": 1, "errors": 1, "files_with_errors": 1 }
}
```

| Field | Meaning |
|---|---|
| `saule_version`, `commit` | Which toolchain issued these verdicts. A binary on `PATH` can lag the tree it was built from, so a stored result can always be attributed. |
| `ok` | True when nothing was reported; mirrors the exit status |
| `files` | Every file examined, **including the clean ones** |
| `path` | The file as given — a bare basename is ambiguous once two `src_dirs` each hold a `main.sau` |
| `span` | Byte offsets into the source, half-open. Absent when the error carries no label. |
| `help`, `code` | Absent rather than `null` when the error has none |

`phase` says which pass produced the diagnostic — one of `io`, `lex`, `parse`,
`import`, `semantic`, `type`, `compile` or `runtime`. It is the classification
available today; `code`, a stable per-error identifier, is wired into the shape
but not yet declared by any error variant.

The document is the same contract the [web playground](/saule/play/) consumes,
so the two cannot drift. `--json` replaces the rendered diagnostics rather than
adding to them, and stdout carries nothing else — warnings and the progress
summary stay on stderr. The exit status is unchanged, so this stays usable as a
CI gate.

## `saule fmt`

Format one or more source files.

```sh
saule fmt [OPTIONS] <FILE>...
```

| Option | Description |
|---|---|
| `-w`, `--write` | Overwrite the files in place instead of printing to stdout |
| `--indent <N>` | Columns per indent level, 1–16 |
| `--tabs` | Indent with hard tabs |
| `--spaces` | Indent with spaces |

At least one `FILE` is required. `--tabs` and `--spaces` override each other, so
the last one on the command line wins and a wrapper script can safely append
one.

Indentation is resolved in layers, each overriding the one before it:

1. The formatter's defaults — spaces, width 2.
2. `indent_style` and `indent_width` from the project's
   [`saule.config`](/saule/language/project-configuration/).
3. The `--indent` / `--tabs` / `--spaces` flags.

Because the language server reads the same `saule.config` keys, a project's
declared style survives both a Reformat in the IDE and a `saule fmt -w` in a
terminal.

Exits with status 1 if any file failed to format.

## `saule init`

Scaffold a new project in `./<name>`.

```sh
saule init [--lib] <NAME>
```

| Option | Description |
|---|---|
| `--lib` | Scaffold a library — importable by other projects, with no entry point |

`NAME` is both the directory that gets created and the project's `name:` in
`saule.config`, which is the prefix dependents use to import from it.
