## Install

```sh
curl -fsSL https://lauriszz123.github.io/saule/install.sh | sh
```

```powershell
irm https://lauriszz123.github.io/saule/install.ps1 | iex
```

Then `saule --version` should print `saule {{VERSION}}`.

The installer picks the build for the machine, verifies it against
`SHA256SUMS`, installs both binaries into `~/.saule/bin`
(`%USERPROFILE%\.saule\bin` on Windows) and puts that directory on the
`PATH`. Running it again upgrades in place.

### Installing by hand

Download the archive for the platform below, verify it against
`SHA256SUMS`, and put **both** binaries somewhere on the `PATH`:

```sh
shasum -a 256 -c SHA256SUMS --ignore-missing
tar xzf saule-{{VERSION}}-<triple>.tar.gz
mkdir -p ~/.saule/bin
cp saule-{{VERSION}}-<triple>/saule saule-{{VERSION}}-<triple>/saule-lsp ~/.saule/bin/
```

Then add `~/.saule/bin` to the `PATH`.

`saule-lsp` is the language server every editor plugin uses; without it on
`PATH` there is syntax highlighting and indentation but no diagnostics, hover,
or formatting.

On macOS, if the archive was downloaded in a browser rather than with `curl`,
clear the quarantine flag: `xattr -dr com.apple.quarantine ~/.saule/bin`.
