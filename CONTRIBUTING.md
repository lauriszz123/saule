# Contributing to Saule

Bug reports and pull requests are welcome. For anything large, please open an
issue first, so nobody spends time on a change that does not fit.

## Reporting a bug

Include what was run, what was expected, what happened, and the output of
`saule --version`. The smallest program that shows the problem helps most. For a
security problem, see [SECURITY.md](SECURITY.md) instead.

## Building and testing

Building needs [Rust](https://rustup.rs) 1.98 or newer.

The example projects live in a repository of their own,
[saule-examples](https://github.com/lauriszz123/saule-examples), which this
one carries as the `examples/` folder (a git submodule). They are part of
the tests, so fetch them too:

```sh
git clone --recurse-submodules https://github.com/lauriszz123/saule.git
# or, in an existing clone:
git submodule update --init
```

A change to an example is a commit in saule-examples, and then a commit
here that moves `examples/` to it.

```sh
cargo build
cargo test --workspace
./run_tests.sh      # the `.sau` fixtures in tests/
./run_examples.sh   # the example projects
cargo fmt --all
cargo clippy --workspace --all-targets
```

To use a local build in an editor, turn on the plugin's setting for a project's own
build. See each plugin's README.

## Where things go

- **A new diagnostic** gets a fixture in `tests/errors/<area>/`, which has to fail.
  `crates/saule-lsp/tests/live_server.rs` also requires the editor to flag it.
- **A new language feature** gets a passing fixture in `tests/run/<area>/`, and
  belongs in `LANGUAGE.md`. Its keywords must also reach
  `grammar/saule.tmLanguage.json` and the three editor plugins.
- **A standard library function** needs a signature as well as an implementation,
  so misuse is caught before the program runs, and an entry in `DOCS.md`.

## Pull requests

Open them against `develop`. Tests must pass on Linux, macOS and Windows. `main`
is the release branch, and every push to it publishes a release.

A contribution is licensed under the
[MIT License](LICENSE). Everyone taking part is expected to follow the
[Code of Conduct](CODE_OF_CONDUCT.md).
