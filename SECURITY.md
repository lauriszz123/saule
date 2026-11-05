# Security

## Reporting a vulnerability

Please report it privately through GitHub:
[**Report a vulnerability**](https://github.com/lauriszz123/saule/security/advisories/new).
Do not open a public issue. Saule is maintained by one person, so expect a first
reply within about a week.

Fixes go into the newest release only. Versions are not patched separately, so
updating is how you get a fix.

## What Saule trusts

Some of this is by design, and worth knowing before you run code you did not
write.

- **A Saule program has the same access as any program you run.** It can read
  and write files and start processes through `Os` and `Io`. There is no
  sandbox, except in the browser playground, which has no access to your machine.
- **`saule install` runs code.** A native package is built with
  `cargo build --release`, which runs that package's build script, and its
  library is loaded when a program imports it. Install packages from people you
  trust. `saule.lock` pins the commit you reviewed, but it does not review it.
- **Packages have no signatures and no central registry.** A package is a Git
  repository, so you are trusting that repository and GitHub.
- **Checking and editing do not run your code.** `saule check`, `saule fmt` and
  `saule-lsp` read source files without running them or loading native packages.
- **Editor plugins do not run binaries from a project.** They use your installed
  toolchain unless you turn on the setting to use a project's own `target/`
  build. Leave it off unless you trust every project you open.

## Installing

`install.sh` and `install.ps1` check each download against the `SHA256SUMS` file
from the same release. That catches a corrupted download, but not a compromised
release. Release binaries are not code-signed, so Windows and macOS may warn when
you download one in a browser.
