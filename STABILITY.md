# Stability

**Saule is young and still changing. Do not build something you cannot easily
update on it yet.** This page says what you can rely on today.

## Versions

A version is `<year>.<build>`, such as `26.20`. Every release gets the next build
number, and new features and fixes ship together. There is no separate patch
line: to get a fix, update to the newest release.

## What may change

Until Saule declares a 1.0:

- **The language and standard library can change between releases.** That
  includes changes that break existing programs, such as a stricter type check.
- **Native packages are tied to an ABI version.** One built for another ABI is
  refused with an explanation, and has to be rebuilt.
- **Bytecode is internal.** Nothing should depend on `saule disasm` output or on
  the opcodes.

Breaking changes are listed in [CHANGELOG.md](CHANGELOG.md).

## What you can do about it

- Set `min_saule_version:` in `saule.config`, so an older toolchain refuses your
  project rather than misreading it.
- Commit `saule.lock`, so your packages stay at the commits you tested.
- Run `saule check` and `saule test` in CI, against the version you mean to use.
