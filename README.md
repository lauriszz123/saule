<img src="brand/png/saule-logo-512.png" alt="Saule" width="128" align="right">

# Saule

A statically typed, class-oriented scripting language with Lua's simplicity and
runtime model — minimal to write, checked before it runs.

**[Website](https://lauriszz123.github.io/saule/)** ·
**[Playground](https://lauriszz123.github.io/saule/play/)** ·
**[Language guide](./LANGUAGE.md)** ·
**[Standard library](./DOCS.md)** ·
**[Changelog](./CHANGELOG.md)**

---

## Install

**macOS and Linux**

```sh
curl -fsSL https://lauriszz123.github.io/saule/install.sh | sh
```

**Windows**

```powershell
irm https://lauriszz123.github.io/saule/install.ps1 | iex
```

Then open a new terminal and check it:

```sh
saule --version
```

The installer picks the right build for your machine, verifies it against the
release's `SHA256SUMS`, and installs `saule` and `saule-lsp` into `~/.saule/bin`.
Running it again upgrades in place.

That installs the **stable** channel. For the newest build of all, pass
`SAULE_CHANNEL=latest` (`$env:SAULE_CHANNEL='latest'` on Windows) — your choice
is remembered, so later upgrades stay on it. Full details, options and a
build-from-source path are in the
[installation guide](https://lauriszz123.github.io/saule/guides/installation/).

## A taste

```luau
class Player
    name: string              -- public field
    local health: integer     -- private: `local` is the access modifier

    static maxHealth: integer = 100

    fn init(name: string)
        self.name = name
        self.health = Player.maxHealth
    end

    fn damage(amount: integer)
        self.health = self.health - amount
    end

    fn isAlive() -> boolean
        return self.health > 0
    end

    fn status() -> string
        return self.name .. ": " .. self.health .. " HP"
    end
end

local p = Player("Arthur")
p.damage(40)
println(p.status())   -- Arthur: 60 HP
println(p.isAlive())  -- true
```

Types are checked before a single line executes, `integer` and `float` never
convert implicitly, and a variable cannot hold `nil` unless its type says `?`:

```luau
local nickname: string? = nil
local display: string = nickname ?? "no nickname"
```

The [language guide](./LANGUAGE.md) covers classes, interfaces, enums, pattern
matching, generics, error handling and the module system.

## Commands

| Command | What it does |
|---|---|
| `saule run <file\|project>` | Run a script or a project |
| `saule check` | Typecheck without executing |
| `saule test` | Run every `fn test_*` in `tests/` and `*_test.sau` |
| `saule fmt` | Format source in place |
| `saule init [--lib]` | Scaffold a project or a library |
| `saule install` / `update` / `remove` / `list` | Manage dependencies |
| `saule disasm` | Show the bytecode |

## Packages

A package is a public Git repository with a `saule.config` — there is no
registry and no central index.

```sh
saule install lauriszz123@saule-shine         # the newest tagged version
saule install lauriszz123@saule-shine:v0.1.0  # a specific tag or branch
saule install ../uikit                        # a project on this machine
saule install                                 # everything in saule.config
```

A package is `<owner>@<repo>`, optionally pinned with `:<version>`. It is
installed once, globally, and shared by every project. Dependencies resolve to
exact commits recorded in `saule.lock` — commit that file. A library declares
its own import name, so where it lives is independent of what you type to
import it, and publishing one means pushing a Git tag.

A path dependency is different in kind: it is used *where it lies*, so your
edits take effect immediately. That is what to reach for with a library you are
developing alongside the project that uses it.

## Editor support

| Editor | Install |
|---|---|
| VS Code | [Marketplace](https://marketplace.visualstudio.com/items?itemName=saule.saule) — `saule.saule` |
| JetBrains IDEs | [Marketplace](https://plugins.jetbrains.com/plugin/35823-saule) |
| Neovim | [`lauriszz123/saule-nvim`](https://github.com/lauriszz123/saule-nvim) |

All three use `saule-lsp`, which the installer puts on your `PATH`, and give
diagnostics, hover, go-to-definition, references, symbols, inlay hints,
signature help, completion and formatting.

## Status

Saule is young and still changing. **Do not build something you cannot easily
update on it yet** — see [STABILITY.md](./STABILITY.md) for what may change and
[CHANGELOG.md](./CHANGELOG.md) for what already has.

Programs run on a bytecode VM at 0.8×–4.8× the speed of PUC Lua. The tree-walking
interpreter is kept in-tree as a differential-testing oracle: every fixture and
example project runs under both engines and the output is compared.

## Contributing

Bug reports and pull requests are welcome — see
[CONTRIBUTING.md](./CONTRIBUTING.md). For a security problem, read
[SECURITY.md](./SECURITY.md) instead. Everyone taking part follows the
[Code of Conduct](./CODE_OF_CONDUCT.md).

## License

[MIT](./LICENSE).
