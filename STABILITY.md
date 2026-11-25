# Stability

**Saule is young and still changing. Do not build something that cannot easily
be updated on it yet.** This page says what can be relied on today.

## Versions

A version is `<year>.<build>`, such as `26.20`. Every release gets the next build
number, and new features and fixes ship together. There is no separate patch
line: to get a fix, update to the newest release.

## Channels

The installer offers two, and the difference is which releases it will
offer — not a different version scheme. `26.20` means the same build on both.

- **`stable`** (the default) — the newest release blessed as stable.
- **`latest`** — the newest release of any kind, which is whatever the last
  push to `main` built.

**A build is blessed as stable once it has been the `latest` release for a week
with no regression reported against it.** That is a deliberately low bar for a
pre-1.0 toolchain, and it is the honest one: it means somebody has run it,
not that it has been proven. It will tighten when the
backwards-compatibility gates land.

Because there is no patch line, **a bug found on `stable` is fixed by blessing
a newer build, not by patching the one installed.** So `stable` is a slower
channel, never a separately maintained one — which is what keeps it compatible
with "to get a fix, update to the newest release" above. If a fix is needed
before the next blessing, move to `latest` or pin that version with
`SAULE_VERSION`.

## What may change

Until Saule declares a 1.0:

- **The language and standard library can change between releases.** That
  includes changes that break existing programs, such as a stricter type check.
- **Native packages are tied to an ABI version.** One built for another ABI is
  refused with an explanation, and has to be rebuilt.
- **Bytecode is internal.** Nothing should depend on `saule disasm` output or on
  the opcodes.

Breaking changes are listed in [CHANGELOG.md](CHANGELOG.md).

## What can be done about it

- Set `min_saule_version:` in `saule.config`, so an older toolchain refuses the
  project rather than misreading it. It compares version numbers only, and
  knows nothing about channels — a `26.20` is a `26.20` wherever it came from.
- Stay on the `stable` channel, and say which channel is in use in a bug
  report.
- Commit `saule.lock`, so packages stay at the commits that were tested.
- Run `saule check` and `saule test` in CI, against the version meant to be used.
