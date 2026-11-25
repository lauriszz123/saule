# Security

## Reporting a vulnerability

Please report it privately through GitHub:
[**Report a vulnerability**](https://github.com/lauriszz123/saule/security/advisories/new).
Do not open a public issue. Saule is maintained by one person, so expect a first
reply within about a week.

Fixes go into the newest release only. Versions are not patched separately, so
updating is how a fix is obtained.

## What Saule trusts

Some of this is by design, and worth knowing before running code from
someone else.

- **A Saule program has the same access as any other program.** It can read
  and write files and start processes through `Os` and `Io`. There is no
  sandbox, except in the browser playground, which has no access to the machine.
- **`saule install` runs code.** A native package is built with
  `cargo build --release`, which runs that package's build script, and its
  library is loaded when a program imports it. Install packages only from trusted
  authors. `saule.lock` pins the commit that was reviewed, but it does not review it.
- **Packages have no signatures and no central registry.** A package is a Git
  repository, so installing one means trusting that repository and GitHub.
- **Checking and editing do not run code.** `saule check`, `saule fmt` and
  `saule-lsp` read source files without running them or loading native packages.
- **Editor plugins do not run binaries from a project.** They use the installed
  toolchain unless the setting to use a project's own `target/`
  build is turned on. Leave it off unless every project opened is trusted.

## Installing

`install.sh` and `install.ps1` check each download against the `SHA256SUMS` file
from the same release. That catches a corrupted download, but not a compromised
release. Release binaries are not code-signed, so Windows and macOS may warn when
one is downloaded in a browser.
