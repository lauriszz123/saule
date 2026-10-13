---
title: "Project Configuration"
description: "Every Saule project has a saule.config file at the root:"
sidebar:
  order: 14
---

<!-- Generated from README.md by `npm run sync-docs`. Edit that file, not this one. -->

Every Saule project has a `saule.config` file at the root:

```
name: "myproject"
version: "1.0.0"
entry: "main.sau"
src_dirs: ["src"]
dependencies: ["../shared-lib", "~/code/json", "lauriszz123@uikit:v1.2.0"]
min_saule_version: "26.1"
indent_style: "space"
indent_width: 2
```

Recognised keys:

| Key | Purpose |
|---|---|
| `name` | Project name; also the import prefix exposed to dependents |
| `version` | Free-form version string (semver recommended) |
| `entry` | Path to the entry `.sau` file, relative to the project root (apps only) |
| `kind` | `"app"` (default) or `"library"` — a library has no entry point and is imported rather than run |
| `src_dirs` | List of directories to search when resolving imports |
| `dependencies` | Packages (`owner@repo`, pinned with `:version`) and paths to other Saule projects (each must itself contain a `saule.config`); `~/` expands to the home directory. Managed with `saule install` / `update` / `remove` — see [Packages](#packages) |
| `min_saule_version` | Refuses to run if the toolchain reports a lower version |
| `indent_style` | Formatting: `"tab"` or `"space"` (default `"space"`) |
| `indent_width` | Formatting: columns per indent level, 1–16 (default `2`) |

Unknown keys are ignored.

#### Packages

A `dependencies:` entry is one of two things, told apart by the `@`:

```
dependencies: ["lauriszz123@uikit:v1.2.0", "../shared-lib"]
```

A **package** is `owner@repo`, optionally pinned with `:version`, and is
fetched from GitHub into `SAULE_HOME` where every project on the machine
shares one copy. A **path** is anything else — absolute, `~`-prefixed, or
relative to the project root — and is used *where it lies*, so nothing is
copied and edits to it take effect immediately.

```sh
saule install lauriszz123@uikit          # the newest tagged version
saule install lauriszz123@uikit:v1.2.0   # a specific tag or branch
saule install ../shared-lib              # a project on this machine
saule install                            # everything this project records
saule update                             # move packages to their newest tag
saule update --check                     # report what is behind, change nothing
saule remove uikit                       # drop it from this project
saule list                               # what this project uses
```

Leaving the version off installs the newest tag and then **records it**, so a
fresh clone of the project gets that same version rather than whatever is
newest that day. Pre-releases are passed over unless you pin one. A repository
with no tags falls back to its default branch, and `saule update` compares such
a package by commit.

An entry always holds an exact version — there are no ranges, so `saule update`
is the only thing that ever changes one. `--check` exits 1 when something is
behind and 2 when a package could not be reached, so CI can tell the two apart.

A package's **import name** comes from its own `name:`, not from its repository
name, which is why Saule needs no central index. `saule install` prints the
`import` line to use.

The two `indent_*` keys are what `saule fmt` and the language server both read,
so a project's declared style survives a Reformat in the IDE and a `saule fmt -w`
in a terminal alike. They override the editor's own Code Style settings; the
`saule fmt --indent <n>` / `--tabs` / `--spaces` flags override them in turn.

### Recommended Project Structure

```
myproject/
├── saule.config
├── main.sau
├── entities/
│   ├── Entity.sau
│   ├── Player.sau
│   └── Enemy.sau
├── data/
│   ├── Repository.sau
│   └── PlayerRepository.sau
├── utils/
│   ├── Math.sau
│   └── Logger.sau
└── enums/
    ├── Direction.sau
    └── Status.sau
```

### Entry Point

There are two ways to run Saule code, with different rules about what the entry file must contain:

**Project mode** — `saule run` in a directory containing `saule.config`, or `saule run <dir>` naming one. The file pointed to by `entry:` must declare:

```saule
class Main
    static fn main()
        -- your code here
    end
end
```

Top-level statements in the entry file still execute first (handy for one-off setup or imports), and then `Main.main()` is called. Without a `Main` class the runner exits with `error: '<entry>' must declare 'class Main' with a 'static fn main()' entry point`.

**Single-file mode** — `saule run path/to/file.sau`, naming a file rather than a directory. The file is executed top-to-bottom like a Lua script; no `class Main` is required, and any surrounding `saule.config` is ignored. If the script happens to define a `Main` with a `static fn main()`, it is invoked as a convenience after the top-level body finishes.

Whether the target is a directory is the *only* thing that picks between the two modes. Arguments for the program itself go after `--`, where the CLI passes them through untouched to `Os.args()`:

```sh
saule run -- input.bf          # project in the cwd, Os.args() = ["input.bf"]
saule run tool.sau -- -v file  # single file; script args may start with `-`
```

### Execution

`saule run` compiles the whole program — the entry file and every module it
imports — to bytecode, then runs it on a register-based virtual machine.
Everything is checked before anything runs: a type error in an imported
module is reported against the `import` that led to it, and so is a module
that cannot be found or parsed.

A typical project-mode entry file:

```saule
import Player from "entities/Player"
import Math from "utils/Math"

class Main
    static fn main()
        local p: Player = Player("Arthur", 100, 1.5)
        p.greet()

        local dmg: integer = Math.clamp(50, 0, 100)
        p.damage(dmg)
    end
end
```

---
