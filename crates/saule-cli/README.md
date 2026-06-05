# saule-cli

The `saule` command-line driver — the main entry point for running and
managing Saule programs. `main.rs` does only arg parsing and dispatch; the
work lives in submodules.

## Commands

```text
saule run <file.sau> [args...]   run a single Saule source file
saule run [args...]              run the project in the current directory
saule run -- [args...]           force project mode, forward args to Os.args()
saule fmt <file.sau> ...         print formatted source to stdout
saule fmt -w <file.sau> ...      overwrite files in place
saule init <name>                scaffold a new Saule project in ./<name>
saule --help | -h
saule --version | -V
```

| Module    | Responsibility                                         |
|-----------|--------------------------------------------------------|
| `run`     | File / project execution (lex → parse → typeck → `Main`) |
| `project` | `saule.config` parsing and project-mode bootstrap      |
| `init`    | Project scaffolding                                    |
| `fmt`     | `saule fmt` front-end over `saule-fmt`                 |

A single-file run invokes `Main.main()` when present; project mode requires
it. Trailing args are exposed to scripts via `Os.args()`.

## Dependencies

`saule-ast`, `saule-lexer`, `saule-parser`, `saule-semantic`,
`saule-interpreter`, `saule-fmt`.
