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
| Reformat on save (on by default) | `com.saule.lang.format.SauleFormatOnSave` |
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

**Editor ▸ Code Style ▸ Saule ▸ Tabs and Indents** decides how `.sau` is laid
out for projects that don't declare a style of their own (see *Project-wide
indentation* below). It defaults to `saule fmt`'s canonical style — 2 spaces —
but *Use tab character* (with *Smart tabs*, *Tab size*, *Indent* and
*Continuation indent*) is a first-class choice, and everything follows it:

* **Typing.** Enter indents the new line from the enclosing `class` / `fn` /
  `if` / `match` block, and `end`, `else`, `elseif`, `until`, `catch` and `case`
  pull back out a level as you type them. Computed by `SauleIndentModel`, which
  mirrors the printer in `crates/saule-fmt` — keep the two in step.
* **Reformat** (`Ctrl+Alt+L`, or Actions on Save). `SauleFormattingFeature`
  reads the same options off the file and sends them as the `insertSpaces` /
  `tabSize` of the `textDocument/formatting` request, and `saule-lsp` feeds
  them straight into `FmtOptions`.

* **Save.** Every `.sau` file is reformatted as it is written — Ctrl+S, *Save
  All*, IdeaVim's `:w`, or the autosave that fires when the IDE loses focus.
  This is on by default; the checkbox lives in **Settings ▸ Tools ▸ Actions on
  Save** ("Reformat Saule files") and is mirrored on the Saule settings page.

  It is a Saule action rather than the platform's own *Reformat code* on save,
  because that one can't work here: it starts LSP4IJ's **asynchronous** format
  and the file is written before the server's edits come back — the file looks
  unformatted, and the buffer quietly goes modified again a moment later.
  `SauleFormatOnSave` sets the platform's `FORMAT_DOCUMENT_SYNCHRONOUSLY` flag
  and formats from inside a write action, so the edits are applied inline and
  the bytes that reach disk are the formatted ones. Leaving the platform's
  *Reformat code* box ticked as well is harmless — it just reformats
  already-formatted text.

Note that LSP has a single `tabSize` where the page has both **Indent** and
**Tab size**. The request carries whichever of the two decides the width of one
level — *Indent* for spaces, *Tab size* when *Use tab character* is on — so the
two only have to agree if you want the editor and a tab-rendering elsewhere to
line up. (Older builds of this plugin left that to LSP4IJ, which always sent
*Tab size*; a page set to indent 4 / tab size 2 formatted to 2. Requires
LSP4IJ 0.20 or newer.)

### Project-wide indentation

A style set in Code Style lives in *your* IDE, so a teammate's Reformat — or a
`saule fmt -w` from a terminal — can still pull the files back to something
else. To settle it for everyone, declare it in the project's `saule.config`:

```text
indent_style: "tab"   -- or "space"
indent_width: 4       -- columns, 1..=16
```

`saule-lsp` applies those on top of whatever LSP4IJ sent, so the config wins
over the Code Style page for files inside that project, and `saule fmt`
reads the same keys. Reformat and the CLI then produce identical files.

The CLI can also be pointed at a style directly, which overrides the config
for that run:

```bash
saule fmt -w --tabs --indent 4 src/TestPanel.sau
```

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
│  │  ├─ format/        SauleIndentModel + LineIndentProvider + FormatOnSave
│  │  ├─ lexer/         SauleLexer + SauleTokenTypes
│  │  ├─ highlight/     SyntaxHighlighter (+factory) + ColorSettingsPage
│  │  └─ lsp/           SauleLanguageServerFactory + SauleLspLocator
│  └─ resources/
│     ├─ META-INF/plugin.xml
│     └─ icons/saule.svg
└─ src/test/kotlin/     SauleIndentModelTest (`./gradlew test`)
```

[lsp4ij]: https://plugins.jetbrains.com/plugin/23257-lsp4ij
