---
title: "Saule"
description: "The version of the toolchain running your code. Distinct from Project.version, which is the version of the code being run."
sidebar:
  order: 7
---

<!-- Generated from DOCS.md by `npm run sync-docs`. Edit that file, not this one. -->

The version of the toolchain running your code. Distinct from `Project.version`,
which is the version of the code being run.

Saule versions are `<two-digit year>.<build number>` — `26.7` is the seventh
release cut in 2026. There is no patch component; a fix is simply the next
build number. Build numbers restart each year, and comparisons still work
because the year leads: `27.1` is newer than `26.412`.

### Constants

| Name | Type | Description |
| --- | --- | --- |
| `Saule.version` | `string` | `"26.7"` — the version as a version. Compare against this. |
| `Saule.full` | `string` | `"26.7"`, or `"26.8-dev+1a2b3c4"` for a development build. Display only — never parse it. |
| `Saule.year` | `integer` | `26`. |
| `Saule.build` | `integer` | `7`. Counts from 1 within the year; `0` means the version could not be determined. |
| `Saule.isDev` | `boolean` | `false` only when built from a clean release tag. |
| `Saule.commit` | `string` | Short commit hash, or `""` when it was built without git available. |

### Functions

| Signature | Description |
| --- | --- |
| `Saule.atLeast(version: string) -> boolean` | Is this toolchain `version` or newer? Compares dotted numeric components, so `"26.7"` satisfies `"26"` and `"26.7"` but not `"26.8"`. |

```saule
if Saule.atLeast("26.4") then
    println("running on " .. Saule.version)
end
```

`atLeast` is the runtime counterpart to `min_saule_version` in `saule.config`.
The two use the same comparison, but they answer different questions:
`min_saule_version` refuses to run the project at all on an old toolchain,
whereas `atLeast` lets code use a newer facility when it is available and fall
back when it isn't. Reach for `min_saule_version` when your project simply
cannot work without a version, and `atLeast` when it can degrade.

A development build reports the version it is *heading toward*, not the last
one released — so `26.8-dev` satisfies `atLeast("26.8")`. That is deliberate:
it lets you write and test code against a feature before its release exists.

---
