# saule-cli

The `saule` command-line driver — the main entry point for running and
managing Saule programs. The command surface is declared with `clap` in
`cli.rs`; `main.rs` does only dispatch, and the work lives in submodules.

## Commands

```text
saule run                        run the project in the current directory
saule run <dir>                  run the project rooted at <dir>
saule run <file.sau>             run a single Saule source file
saule fmt <file.sau> ...         print formatted source to stdout
saule fmt -w <file.sau> ...      overwrite files in place
saule init <name>                scaffold a new Saule project in ./<name>
saule --help | -h                also available per subcommand
saule --version | -v | -V
```

### `run`: project mode vs single-file mode

```text
saule run [TARGET] [ARGS...] [-- ARGS...]
```

One thing decides the mode: **whether `TARGET` is a directory.** Absent or a
directory → project mode; a file → single-file mode. Nothing inspects file
extensions and nothing probes for a `saule.config` to guess.

The **first** positional is the target and **every one after it** is the
script's own `Os.args()` — the way `python`, `node` and `lua` are all
invoked:

```sh
saule run tool.sau input.md    # single file, Os.args() = ["input.md"]
saule run bf/ input.bf         # project in ./bf, Os.args() = ["input.bf"]
```

Everything after `--` is script argv too, passed through verbatim and never
interpreted by the CLI. Two cases still need it: giving argv to the project
in the *current* directory, where the first positional would otherwise be
read as the target, and passing an argument that begins with `-`, which
would otherwise be one of this command's own flags:

```sh
saule run -- input.bf          # project in the cwd, Os.args() = ["input.bf"]
saule run tool.sau -- -v file  # single file; script args may start with `-`
```

The two spellings concatenate in the order written, so `saule run t.sau a --
-v` reaches the script as `["a", "-v"]`.

### `fmt` indentation

| Flag | Effect |
|---|---|
| `--indent <n>` | Columns per indent level, 1–16 (`--indent=4` also works) |
| `--tabs` | Indent with hard tabs |
| `--spaces` | Indent with spaces |

Without flags, each file follows the nearest `saule.config` above it
(`indent_style:` / `indent_width:`), and files in no project get the
canonical 2 spaces. Flags override the config for that run. See
[`saule-fmt`](../saule-fmt/README.md#indentation-precedence) for how this
lines up with the options an editor sends the language server.

| Module    | Responsibility                                         |
|-----------|--------------------------------------------------------|
| `cli`     | The `clap` command surface — definitions only          |
| `run`     | File / project execution (lex → parse → typeck → `Main`) |
| `project` | `saule.config` parsing and project-mode bootstrap      |
| `init`    | Project scaffolding                                    |
| `fmt`     | `saule fmt` front-end over `saule-fmt`                 |

A single-file run invokes `Main.main()` when present; project mode requires
it. Trailing args are exposed to scripts via `Os.args()`.

## Dependencies

`saule-ast`, `saule-lexer`, `saule-parser`, `saule-semantic`,
`saule-interpreter`, `saule-fmt`.
