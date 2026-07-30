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
