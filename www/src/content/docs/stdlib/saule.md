---
title: "Saule"
description: "The version of the toolchain running your code. Distinct from Project, which is the version of the code being run: upgrade the compiler and…"
sidebar:
  order: 9
---

<!-- Generated from DOCS.md by `npm run sync-docs`. Edit that file, not this one. -->

The version of the **toolchain** running your code. Distinct from
[`Project`](/saule/stdlib/project/), which is the version of the code being run: upgrade
the compiler and `Saule.version` moves, while `Project.version` only moves
when you edit your own config.

Saule versions are `<two-digit year>.<build number>` — `26.7` is the seventh
release cut in 2026. There is no patch component; a fix is simply the next
build number. Build numbers restart each year, and comparisons still work
because the year leads: `27.1` is newer than `26.412`.

| Name | Type | What it holds |
| --- | --- | --- |
| `Saule.version` | `string` | The version as a version — `"26.7"`. Compare against this |
| `Saule.full` | `string` | Display only: `"26.7"`, or `"26.8-dev+1a2b3c4"` for a development build. Never parse it |
| `Saule.year` | `integer` | The year part — `26` |
| `Saule.build` | `integer` | The build number within the year, counting from 1. `0` when it could not be determined |
| `Saule.isDev` | `boolean` | `false` only when built from a clean release tag |
| `Saule.commit` | `string` | Short commit hash, or `""` when built without git available |

```saule
println(#Saule.version > 0)            -- true
println(Saule.year >= 26)              -- true
println(Saule.build >= 0)              -- true

-- `full` is for showing a human; `version` is for comparing.
println(String.starts(Saule.full, Saule.version))   -- true
```

<div class="api-list">
<details class="api-entry" id="saule-atleast">
<summary><code class="api-sig"><span class="sig-fn">Saule.atLeast</span><span class="sig-punct">(</span><span class="sig-param">version</span><span class="sig-punct">:</span> <span class="sig-type">string</span><span class="sig-punct">)</span> <span class="sig-arrow">-&gt;</span> <span class="sig-type">boolean</span></code></summary>


Whether this toolchain is `version` or newer. Compares dotted numeric
components, so `"26.7"` satisfies `"26"` and `"26.7"` but not `"26.8"`.

A development build reports the version it is *heading toward*, not the last
one released — so `26.8-dev` satisfies `atLeast("26.8")`. That is deliberate:
it lets you write and test code against a feature before its release exists.

```saule
println(Saule.atLeast("26"))           -- true
println(Saule.atLeast("1.0"))          -- true, long past it
println(Saule.atLeast("99.1"))         -- false

-- Using a newer facility when it is there, and degrading when it is not.
if Saule.atLeast("26.4") then
    println("running on " .. Saule.version)
else
    println("older toolchain")
end
```

`atLeast` is the runtime counterpart to `min_saule_version` in `saule.config`.
The two use the same comparison but answer different questions:
`min_saule_version` refuses to run the project at all on an old toolchain,
whereas `atLeast` lets code degrade. Reach for `min_saule_version` when your
project simply cannot work without a version, and `atLeast` when it can.

</details>
</div>

