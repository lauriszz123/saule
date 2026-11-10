# Changelog

Newest first. Releases not listed here had no notes written for them; see
[GitHub Releases](https://github.com/lauriszz123/saule/releases) and the commit
history.

## Next

### Added

- **Coroutines.** `Coroutine.create(fn(co) … end)` makes a function that pauses
  at `co.yield(v)` and resumes from exactly there, with its locals and call
  stack intact. They are stackful — pass the yielder down and any function can
  yield — and typed: a `Coroutine<T>` yields `T`s, `resume()` answers a `T?`,
  and `for v in gen do` iterates one with `v: T`. `T` is learned from where the
  coroutine goes, so the body's `co` needs no annotation. A coroutine that
  fails is reported through `resume()` and `error()` rather than ending the
  program. See [Coroutines](./LANGUAGE.md#coroutines) and the
  [`Coroutine` reference](./DOCS.md#coroutine); the editor types, completes and
  documents both handles.

### Changed

- **A generic class's methods are checked like any other's.** On a receiver
  such as `Box<integer>`, `box.put("x")` is now an argument error, a misspelt
  method is reported, and a lambda argument's parameters take their types from
  the receiver. These calls were not checked at all before.
- **A generic method learns its type parameters from where its result goes**
  when the arguments leave them open — an annotated `local`, a declared return
  type, or the parameter the call fills. An argument that disagrees is still
  reported.
- `for v in x` over an instance of an `Iterable<T>` class binds `v` as a `T`
  when it is not annotated, as it already did for tables.
- A method call on an `any` value is `any`, as a field read on one already
  was, so a dynamic call's result can be used anywhere `any` can.

### Fixed

- A callback run by a built-in — a `Table.sort` comparator, an operator
  overload, a `toString` — reads and writes the enclosing function's locals
  correctly. It used to touch its own registers instead: the caller never saw
  the writes, and the callback could fail with an internal type error.
- `import uikit` followed by a `from …` line still being typed no longer
  reports the import as the removed syntax, and completion works on the `from`
  line. An unfinished `from path` is reported on its own line.

## 26.24

### Changed

- **`import shine` brings in every export, as `from shine import *` does**, and
  keeps `shine.Window` working too.

### Fixed

- Completion after `from uikit ` offers `import`, and after `import uikit ` or
  `from uikit import Window ` offers `as`, instead of every name in scope.
  `from` is offered where a statement begins.
- Completion ranks what fits first in more places: a local without a type
  annotation ranks by the value it was given, a standard-library argument by
  its parameter's type, an `if`, `while` or `until` condition puts booleans
  first, and a type reached through a namespace (`uikit.ThemeData`) fits a slot
  declared with the bare name. Where no type applies, names spelled the way
  you started typing come first: `f` reaches `fn`, `for` and your locals before
  the `FlowStack` class.

## 26.23

### Changed

- **Imports name the module first.** `from shine import Window, Button as Btn`
  and `from "uikit" import *` replace `import Window from shine` and
  `import * from "uikit"`, so the editor knows what it can offer before you type
  a name. The old form is now a syntax error that says what to write instead.
- **Re-exports are written out.** `export from shapes import *` passes names on,
  from any file. An `init.sau` no longer re-exports whatever it imports: a plain
  `from … import` keeps its names private there too. Turn each import in a
  folder's `init.sau` into `export from` to keep its surface as it was.

### Added

- **Namespace imports.** `import shine`, `import "ui/widgets"` or
  `import shine as sh` bring in a module as a whole; `shine.Window` then works as
  a name and as a type, with completion after `shine.`, hover, go to definition
  and references. A name the module does not export reads as
  `` `shine` has no export named `Windw` ``.

### Fixed

- A function brought in by `from x import *` can be called from a file's top
  level. It was rejected as used before its declaration.

## 26.22

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

### Changed

- **The README is a front door, not the manual.** The language guide it used to
  hold — 3,900 lines of it — is now [LANGUAGE.md](LANGUAGE.md), and the README
  is install, a taste of the language, and where to go next. The website is
  generated from `LANGUAGE.md` and its pages are unchanged. Release archives
  carry `README.md`, `LANGUAGE.md` and `DOCS.md`, so an offline copy is still
  the whole manual.

### Fixed

- **`Io.stdin`, `Io.stdout` and `Io.stderr` are documented** as values with
  types, rather than only mentioned in the prose around them.
- **`Math.pow`'s parameters are called `base` and `exp`**, not `a` and `b`.
- **`String.find`'s second argument is `needle`, not `pattern`** in signature
  help. It takes literal text; the name was merely unhelpful before
  `String.gsub` existed and wrong once it did.
- **`local finally = 1` says why it is rejected.** `finally` was reserved by the
  parser but missing from the list the token helpers consult, so a name
  collision with it produced a bare parse error instead of "`finally` is a
  reserved word — choose another name", and `obj.finally` would not parse as a
  member access although `obj.try` did.
- **`saule install` explains a mingw toolchain.** A native package whose
  dependencies reach `windows-sys` cannot link on a Windows host whose default
  Rust toolchain targets `x86_64-pc-windows-gnu` without mingw's `dlltool`
  installed. The failure was reported as "the package failed to build", leaving
  the reader to decode a linker error whose cause was never the package. It now
  names the toolchain, says so, and gives the one-line fix.
- **Two dead links in the standard-library reference.** `Bytes.of` and
  `Bytes.byte` pointed at `../README.md#the-byte-type`, which the website's
  link rewriter did not match and published verbatim.

## 26.21

### Fixed

- **`saule fmt` keeps what you wrote.** `string[]` no longer becomes
  `table<integer, string>`, and neither do `T[,]`, `T[]?` or `(fn() -> T)[]`. String
  escapes stay escapes, `["key"]` keeps its brackets, and `(a) | (b) | c` gains no
  extra parentheses. A comment at the top of a lambda argument stays inside it, and
  a long `f(fn() … end)` is no longer split on to separate lines.
- **Hover and inlay hints know more types.** A call to an imported function, an
  imported variable, a `for` variable, a match binding and a `catch` variable are
  all typed, so `for tag in self.tags` shows `tag: string` and hover stops saying
  `any` for them. Signature help works on imported functions too.

## 26.20

### Added

- **`saule test`** runs every top-level `fn test_name()` in `tests/` and in files
  ending `_test.sau`.
- **Stack traces** on runtime errors and uncaught `throw`s.
- **`saule.lock`** records the exact commit of each package. `saule install`
  restores it and `saule update` moves it. Commit this file.

### Fixed

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
