---
title: "Project"
description: "The saule.config of the code being run. Distinct from Saule, which is the version of the *toolchain* running it: upgrade the compiler and Saule.version…"
sidebar:
  order: 10
---

<!-- Generated from DOCS.md by `npm run sync-docs`. Edit that file, not this one. -->

The `saule.config` of the code being run. Distinct from
[`Saule`](/saule/stdlib/saule/), which is the version of the *toolchain* running it: upgrade
the compiler and `Saule.version` moves, while `Project.version` only moves
when you edit your own config.

| Name | Type | Description |
| --- | --- | --- |
| `Project.name` | `string` | The project's `name:`. |
| `Project.version` | `string` | Its `version:` — the version of *your* code. |
| `Project.root` | `string` | Absolute path of the directory holding `saule.config`. |
| `Project.srcDirs` | `table<string>` | The `src_dirs:` roots, as absolute paths resolved against `root` — not the relative spellings the config used. |

```saule
-- saule.config:  name: "bf"   version: "0.1.0"   src_dirs: ["src"]
println(Project.name .. " " .. Project.version)   -- bf 0.1.0
println(Project.root)                             -- /home/you/code/bf
println(#Project.srcDirs)                         -- 1
```

Three things worth knowing:

**They are snapshots, taken at startup.** There is no setter, and writing to
the `srcDirs` table changes your copy rather than anything the module loader
will consult.

**Every field is a plain `string`, never `string?`.** In **single-file mode**
— `saule run script.sau`, with no `saule.config` above it — there is no
project, and each field is the **empty string** with `srcDirs` an empty table.
A nullable type would push a `??` onto every use of a value that is almost
always there; an empty string is falsy enough to test directly:

```saule
if Project.name == "" then
    println("running as a single file")
end
```

Project mode is what `saule run .`, `saule run <dir>` and a bare `saule run`
select; naming a file directly is single-file mode even when that file sits
inside a project.

**It needs no import.** Like every other stdlib module, `Project` is in scope
everywhere — see [Prelude](/saule/stdlib/prelude-always-in-scope/). For the config file's
own keys, and which of them are required, see
[README §Project Configuration](/saule/language/project-configuration/).

---
