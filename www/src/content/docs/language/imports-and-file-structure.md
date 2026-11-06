---
title: "Imports and File Structure"
description: "An import names either a single .sau file or a folder module (a directory with an init.sau — see Folder Modules), and the path is relative to the…"
sidebar:
  order: 13
---

<!-- Generated from LANGUAGE.md by `npm run sync-docs`. Edit that file, not this one. -->

### Importing

An import names either a single `.sau` file or a folder module (a directory with an `init.sau` — see [Folder Modules](/saule/language/imports-and-file-structure/#folder-modules-initsau)), and the path is relative to the importing file's directory, then the project's `src_dirs`. The module comes first, so an editor knows what it can offer before you type a name.

```saule
-- one name, several, or one under another name
from "entities/Player" import Player
from "utils/Math" import clamp, lerp
from "data/PlayerRepository" import PlayerRepository as PlayerRepo

-- every exported name
from "entities/Player" import *
```

Or import the module as a whole and reach into it by name:

```saule
import shine                  -- shine.Window, shine.run()
import "ui/widgets"           -- widgets.Button
import shine as sh            -- sh.Window

local w: shine.Window = shine.Window("Demo")
```

A namespace is named after the last part of its path. A path whose last part is not a name, like `"my-lib"`, needs `as`. `shine.Window` works anywhere a name or a type does, and a name the module does not export is reported as one: ``"`shine` has no export named `Windw`"``.

The path may be written **with or without quotes**. Unquoted, `.` separates folders — the two lines below mean exactly the same thing:

```saule
from "some/folder/module" import *
from some.folder.module import *
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
from "json" import Json          -- the `json` package
from "json/lexer" import Parser  -- a specific module inside it
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

A folder becomes a single importable **module** by giving it an `init.sau`. Importing the folder imports that file, so whatever it exports is the folder's public surface.

### Re-exports

`export from` passes names on: they are imported here and exported again, so a folder of files can be consumed as one unit.

```saule
-- some/folder/module/init.sau
-- Paths are relative to this file.
export from view import *
export from button import Button, Toggle as Switch
```

Consumers then import the folder itself and get everything it passes on:

```saule
from some.folder.module import View, Button
import some.folder.module as ui

local view: View = View("Name")
local s: ui.Switch = ui.Switch()
```

Only what is written with `export from` is passed on. A plain `from … import` keeps its names private to the file, in an `init.sau` too, so a file's imports stay an implementation detail. Any file can re-export, and a re-export of a re-export works. A namespace cannot be re-exported; name what to pass on instead.

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

Not everything needs a class. Export standalone functions from a utility
file and import them by name — a file is not a namespace, so the functions
arrive as themselves rather than under the file's name:

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
from "utils/Math" import clamp, lerp

local clamped: integer = clamp(150, 0, 100)     -- 100
local smooth: float = lerp(0.0, 1.0, 0.5)       -- 0.5
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
