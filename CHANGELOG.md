# Changelog

Newest first. Releases not listed here had no notes written for them; see
[GitHub Releases](https://github.com/lauriszz123/saule/releases) and the commit
history.

## Next

### Added

- **`saule test`** runs every top-level `fn test_name()` in `tests/` and in files
  ending `_test.sau`.
- **Stack traces** on runtime errors and uncaught `throw`s.
- **`saule.lock`** records the exact commit of each package. `saule install`
  restores it and `saule update` moves it. Commit this file.

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
