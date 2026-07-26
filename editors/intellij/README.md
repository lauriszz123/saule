# Saule for IntelliJ IDEA

JetBrains plugin for the [Saule](../../README.md) programming language (`.sau`):
native **syntax highlighting**, the full **`saule-lsp` language server**
(diagnostics, hover, go-to-definition, find usages, document symbols, inlay
hints, signature help, and formatting), plus **New Project scaffolding** and
**Run configurations**.

Works in **IntelliJ IDEA Community and Ultimate** (and other JetBrains IDEs),
because it rides on the open-source [LSP4IJ][lsp4ij] client rather than the
Ultimate-only native LSP API.

## How it fits together

| Concern | Provided by |
|---|---|
| Colouring, brace matching, commenting, colour-settings page | Native IntelliJ lexer (`com.saule.lang.lexer.SauleLexer`) |
| Indentation while typing (Enter, `Adjust Indent`, auto-dedent of `end`) | `com.saule.lang.format.SauleIndentModel` |
| Diagnostics, hover, navigation, symbols, inlay hints, signature help, formatting | `saule-lsp` binary, connected via LSP4IJ |
| New Project / New Module scaffolding | `SauleModuleType` + `SauleModuleBuilder` (writes `saule.config`, `src/main.sau`, …) |
| Running scripts & projects | `SauleRunConfigurationType` + producer → `saule run` |

## Creating a project

**File ▸ New ▸ Project… ▸ Saule** (or New Module inside an existing project).
It scaffolds exactly what `saule init` produces:

```
myproject/
├─ saule.config
├─ src/main.sau        (a Greeter + Main.main() entry point)
├─ .gitignore
└─ README.md
```

## Running

Both come from the `saule` CLI (auto-discovered next to `saule-lsp`; override
with `SAULE_PATH` / `-Dsaule.path=`):

* **Gutter icon / right-click ▸ Run** on any `.sau` file. If the file is inside
  a `saule.config` tree it runs the **project** (`saule run` from the project
  root — executes the declared entry point); otherwise it runs that **single
  file** (`saule run <file>`).
* **Run ▸ Edit Configurations… ▸ + ▸ Saule** to create one by hand. Leave
  *Script file* empty for project mode; set it for single-file mode. *Program
  arguments* are forwarded to the script via `Os.args()`.

The server has no semantic-token support, so highlighting is done client-side by
a hand-written lexer that mirrors `crates/saule-lexer`. Everything semantic comes
from the same `saule-lsp` binary the VS Code and Neovim integrations use.

## Indentation, tabs and spaces

**Editor ▸ Code Style ▸ Saule ▸ Tabs and Indents** is the single place that
decides how `.sau` is laid out. It defaults to `saule fmt`'s canonical style —
2 spaces — but *Use tab character* (with *Smart tabs*, *Tab size*, *Indent* and
*Continuation indent*) is a first-class choice, and everything follows it:

* **Typing.** Enter indents the new line from the enclosing `class` / `fn` /
  `if` / `match` block, and `end`, `else`, `elseif`, `until`, `catch` and `case`
  pull back out a level as you type them. Computed by `SauleIndentModel`, which
  mirrors the printer in `crates/saule-fmt` — keep the two in step.
* **Reformat** (`Ctrl+Alt+L`, or Actions on Save). LSP4IJ turns the same options
  into the `insertSpaces` / `tabSize` of the `textDocument/formatting` request,
  and `saule-lsp` feeds them straight into `FmtOptions`.

One wrinkle worth knowing: LSP has a single `tabSize`, and LSP4IJ fills it from
**Tab size**, not **Indent**. `saule fmt` uses it as the indent width, so the two
want to stay equal. Set *Indent* to 4 and leave *Tab size* at 2 and the editor
will indent by 4 while Reformat pulls the file back to 2.

The `saule fmt` CLI has no flags for this yet — it always prints the canonical
2 spaces. If you switch the IDE to tabs, format from the IDE, not the CLI.

## Prerequisites

1. **The `saule-lsp` binary.** Build it from the workspace root:
   ```bash
   cargo build --release -p saule-lsp
   ```
   This produces `target/release/saule-lsp[.exe]`, which the plugin finds
   automatically when you open the Saule project (see *Server discovery* below).

2. **JDK 17+** to build the plugin. **A JetBrains IDE 2024.2+** to run it.

3. **The [LSP4IJ][lsp4ij] plugin.** It is declared as a dependency, so when you
   install this plugin the IDE offers to install LSP4IJ from Marketplace too.

## Build & install

From this directory:

```bash
./gradlew buildPlugin
```

The installable ZIP lands in `build/distributions/saule-intellij-<version>.zip`.
Install it with **Settings ▸ Plugins ▸ ⚙ ▸ Install Plugin from Disk…**, then
restart the IDE.

To hack on the plugin in a sandbox IDE with it preloaded:

```bash
./gradlew runIde
```

## Toolchain discovery

Both binaries (`saule`, `saule-lsp`) are located the same way — first hit wins:

1. `SAULE_PATH` / `SAULE_LSP_PATH` env var (or `-Dsaule.path=` / `-Dsaule.lsp.path=`).
2. **Settings ▸ Languages & Frameworks ▸ Saule** — a *toolchain directory* (the
   folder holding the binaries) or explicit per-binary paths. **Set this for
   projects outside the Saule Cargo workspace** (e.g. `C:\Users\you\IdeaProjects\…`),
   where there is no `target/` build output to discover.
3. `target/{release,debug}/saule[-lsp][.exe]`, found by walking up from the
   project base and every content root.
4. On your `PATH`.

If nothing resolves, the run configuration reports a clear error pointing you to
the settings page (instead of a raw `CreateProcess error=2`).

You can inspect and control the running server from **Settings ▸ Languages &
Frameworks ▸ Language Servers** (the LSP4IJ panel), including a **LSP console**
for tracing JSON-RPC traffic.

## Configuration knobs

| Property | Where | Default |
|---|---|---|
| IDE built against | `gradle.properties` → `platformType`/`platformVersion` | `IC` / `2024.2` |
| Compatibility floor | `gradle.properties` → `pluginSinceBuild` | `242` |
| LSP4IJ version | `gradle.properties` → `lsp4ijVersion` | `0.7.0` |

To target an older IDE (LSP4IJ supports 2023.2+), lower **both**
`platformVersion` (e.g. `2023.2`) and `pluginSinceBuild` (e.g. `232`) together.

> Note: building against 2024.2 emits an advisory that `sourceCompatibility`
> "should" be 21. Bytecode 17 loads fine on the IDE's bundled JBR 21; bump
> `javaVersion` in `gradle.properties` to `21` only if you have a JDK 21 handy.

## Layout

```
editors/intellij/
├─ build.gradle.kts                 IntelliJ Platform Gradle Plugin 2.x
├─ gradle.properties                versions & compatibility range
├─ src/main/
│  ├─ kotlin/com/saule/lang/
│  │  ├─ SauleLanguage / SauleFileType / SauleIcons
│  │  ├─ SauleCommenter / SauleBraceMatcher
│  │  ├─ SauleCodeStyleSettingsProvider   Code Style ▸ Saule page
│  │  ├─ editor/        Enter + typed-char indent handlers
│  │  ├─ format/        SauleIndentModel + LineIndentProvider
│  │  ├─ lexer/         SauleLexer + SauleTokenTypes
│  │  ├─ highlight/     SyntaxHighlighter (+factory) + ColorSettingsPage
│  │  └─ lsp/           SauleLanguageServerFactory + SauleLspLocator
│  └─ resources/
│     ├─ META-INF/plugin.xml
│     └─ icons/saule.svg
└─ src/test/kotlin/     SauleIndentModelTest (`./gradlew test`)
```

[lsp4ij]: https://plugins.jetbrains.com/plugin/23257-lsp4ij
