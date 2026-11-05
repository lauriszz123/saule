# Changelog

Newest first. Releases not listed here had no notes written for them; see
[GitHub Releases](https://github.com/lauriszz123/saule/releases) and the commit
history.

## Next

### Added

- **Lua-style patterns.** `String.match`, `String.gmatch` and `String.gsub` read
  their pattern argument as a pattern rather than literal text: `.` is any
  character, `%d` a digit, `(…)` a capture, `%1` a back-reference. The classes,
  quantifiers, anchors, `%b` and `%f` all behave as Lua's do, with two
  deliberate differences — patterns run over **characters, not bytes**, so `.`
  never splits a multi-byte character; and `%a`/`%l`/`%u`/`%s`/`%w` are
  Unicode-aware while `%d` stays ASCII `0`–`9`. `String.find`, `String.replace`
  and `String.split` are unchanged and still literal. See
  [DOCS.md](DOCS.md#patterns).
- **Two install channels.** `curl … | sh` now installs the **stable** channel:
  the newest release blessed as stable, rather than whatever `main` built an
  hour ago. `SAULE_CHANNEL=latest` takes the newest build of all, and the
  choice is remembered in `~/.saule/channel` so later upgrades stay on it.
  Version numbers are unchanged — `26.22` is `26.22` on either channel,
  because stability is a property of the release and not of the version.
  See [STABILITY.md](STABILITY.md#channels).
- **The editor documents the standard library.** Hovering `String.gsub`, `Math.pi`
  or `String` itself now shows the signature *with parameter names*, what the
  thing is for, the example from the reference, and a link to the published page
  — where before it showed `(string, string, string, integer?) -> (string, integer)`
  and nothing else. Signature help, inlay hints and completion get the names
  from the same place. The source is `DOCS.md`, parsed at build time, so the
  editor cannot describe a function the documentation does not: all **174**
  registered members are covered, and a test fails if a new one arrives
  undocumented.
- **A keyword can be a member name** when written tight against the dot, so
  `String.match(…)` and `obj.class` parse. `super` was already exempt there;
  this is the same rule for every keyword. The adjacency requirement is what
  keeps a half-typed `player.` from swallowing the next line's `local`.
- **`saule test`** runs every top-level `fn test_name()` in `tests/` and in files
  ending `_test.sau`.
- **Stack traces** on runtime errors and uncaught `throw`s.
- **`saule.lock`** records the exact commit of each package. `saule install`
  restores it and `saule update` moves it. Commit this file.

### Fixed

- **`Io.stdin`, `Io.stdout` and `Io.stderr` are documented** as values with
  types, rather than only mentioned in the prose around them.
- **`Math.pow`'s parameters are called `base` and `exp`**, not `a` and `b`.
- **`String.find`'s second argument is `needle`, not `pattern`** in signature
  help. It takes literal text; the name was merely unhelpful before
  `String.gsub` existed and wrong once it did.
- **`saule fmt` keeps what you wrote.** `string[]` no longer becomes
  `table<integer, string>`, and neither do `T[,]`, `T[]?` or `(fn() -> T)[]`. String
  escapes stay escapes, `["key"]` keeps its brackets, and `(a) | (b) | c` gains no
  extra parentheses. A comment at the top of a lambda argument stays inside it, and
  a long `f(fn() … end)` is no longer split on to separate lines.
- **Hover and inlay hints know more types.** A call to an imported function, an
  imported variable, a `for` variable, a match binding and a `catch` variable are
  all typed, so `for tag in self.tags` shows `tag: string` and hover stops saying
  `any` for them. Signature help works on imported functions too.
- An arithmetic expression with many terms no longer hangs. Time doubled or worse
  with every `+ - * / %`, so 18 terms effectively never finished, in `saule run`,
  `saule check` and the editor.

## 26.19

### Changed, may break existing code

- These are now compile errors rather than failures when the line runs: `#`, calling,
  indexing, member access and `for ... in` on a type that does not support them,
  a class missing a method its interface requires, and assigning to an enum variant.
  A program with such a mistake on a line that never runs will no longer compile.
- `saule check` and the editor now also report what the compiler rejects, such as
  unknown named arguments and using a name before its declaration.

### Fixed

- The language server updates files that import a changed module, including when
  the change comes from outside the editor, and when a file is closed unsaved.
- Hover and inlay hints keep working when a file has a syntax error elsewhere.
- Members of generic class values, such as `Box<integer>`, resolve in the editor.
- `self.super(...)` in a class extending `Parent<integer>` type-checks.
- A variadic parameter is a table in the function body, not its element type.
