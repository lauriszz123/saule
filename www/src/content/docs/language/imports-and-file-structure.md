---
title: "Imports and File Structure"
description: "An import names either a single .sau file or a folder module (a directory with an init.sau — see Folder Modules). The path is relative to the importing…"
sidebar:
  order: 13
---

<!-- Generated from README.md by `npm run sync-docs`. Edit that file, not this one. -->

### Importing

An import names either a single `.sau` file or a folder module (a directory with an `init.sau` — see [Folder Modules](/saule/language/imports-and-file-structure/#folder-modules-initsau)). The path is relative to the importing file's directory, then the project's `src_dirs`.

```saule
-- single import
import Player from "entities/Player"

-- import with alias
import PlayerRepository as PlayerRepo from "data/PlayerRepository"

-- import a utility module
import Math from "utils/Math"

-- pull every exported name out of one file
import * from "entities/Player"
```

The path may be written **with or without quotes**. Unquoted, `.` separates folders — the two lines below mean exactly the same thing:

```saule
import * from "some/folder/module"
import * from some.folder.module
```

### Apps and Libraries

A project is one of two shapes, declared by `kind:` in `saule.config`:

| `kind` | Has `entry:` | `saule run` | Purpose |
|---|---|---|---|
| `"app"` (default) | yes | runs it | a program |
| `"library"` | no | refuses, and says why | imported by other projects |

Scaffold either with `saule init`:

```sh
saule init myapp          # an app, with src/main.sau
saule init mylib --lib    # a library, with src/init.sau
```

A library's `src/init.sau` is its public surface — whatever that file exports
is what importers see. Running one is a category error and reports as such
rather than failing on a missing entry file.

### Importing from a Dependency

A project listed in `dependencies:` is reachable by its `name:`. Naming the
dependency on its own imports **the package itself**:

```saule
import Json from "json"          -- the `json` package
import Parser from "json/lexer"  -- a specific module inside it
```

A package exposes itself through an **`init.sau`** in one of its `src_dirs` —
the same [folder module](/saule/language/imports-and-file-structure/#folder-modules-initsau) rule that applies anywhere
else, so there is one convention to learn rather than a special case for
dependencies. A package without one can still have its modules imported by
path, but its name alone won't resolve.

```
json/
├── saule.config          name: "json"
└── src/
    └── init.sau          ← what `import ... from "json"` gets
```

### Folder Modules (`init.sau`)

A folder becomes a single importable **module** by giving it an `init.sau`. That file is a *barrel*: whatever it imports becomes the module's public surface, so a folder of files can be consumed as one unit.

```saule
-- some/folder/module/init.sau
-- Paths are relative to this file. This is all the barrel does: it lists
-- the files whose exports should be visible to importers of the module.
import * from view
import * from button
```

Consumers then import the folder itself and get everything the barrel pulled in:

```saule
import * from some.folder.module

local view: View = View("Name")
local b: Button = Button()
```

Named and aliased imports work against a barrel too:

```saule
import View from some.folder.module
import View as V, Button from some.folder.module
```

Re-exporting is **only** done by `init.sau` / `init.saule`. Any other file keeps its imports private — importing a regular file gives you the names it declared with `export`, never the ones it imported. That keeps a file's imports an implementation detail.

### Exporting

Add `export` before a class, interface, enum, function, or variable to make it accessible from other files:

```saule
export class Player
    -- ...
end

export fn clamp(value: integer, min: integer, max: integer) -> integer
    if value < min then return min end
    if value > max then return max end

    return value
end

export maxPlayers: integer = 8
```

An `export name: T = value` is a **module variable** (see [Variables](/saule/language/variables/)) — a single value shared by every file that imports it, not a copy per importer.

A file without `export` is private to itself — even sibling files in the same folder can't see its declarations. The only way to share code across files is to `export` it and `import` it explicitly.

### Utility Modules

Not everything needs a class. Export standalone functions from a utility file:

```saule
-- utils/Math.sau

export fn clamp(value: integer, min: integer, max: integer) -> integer
    if value < min then return min end
    if value > max then return max end
    return value
end

export fn lerp(a: float, b: float, t: float) -> float
    return a + (b - a) * t
end
```

```saule
import Math from "utils/Math"

local clamped: integer = Math.clamp(150, 0, 100)   -- 100
local smooth: float = Math.lerp(0.0, 1.0, 0.5)    -- 0.5
```

### Visibility Rules

| Situation | Accessible from |
|---|---|
| `export class Foo` | anywhere that imports it |
| `class Foo` without export | only inside the same file |
| `local` field or method | only within the class |
| `static` field or method | via `ClassName.x` anywhere |

### Circular Imports

Saule forbids circular imports at compile time:

```
ERROR: Circular import detected
  Player.sau → Inventory.sau → Player.sau

  Hint: Extract shared types into a separate file
```

---
