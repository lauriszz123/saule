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
dependencies: ["../shared-lib", "~/code/json"]
min_saule_version: "2026.1.0"
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
| `dependencies` | List of paths to other Saule projects (each must itself contain a `saule.config`); `~/` expands to the home directory |
| `min_saule_version` | Refuses to run if the toolchain reports a lower version |
| `indent_style` | Formatting: `"tab"` or `"space"` (default `"space"`) |
| `indent_width` | Formatting: columns per indent level, 1–16 (default `2`) |

Unknown keys are ignored.

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
