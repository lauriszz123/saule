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
- **A worked coroutines example.** [`examples/coroutines`](./examples/coroutines)
  is a six-part tour: lazy stream combinators that are themselves coroutines,
  a stackful tokenizer that yields from two frames below the body, a
  recursive-descent parser that pulls one token at a time, and a cooperative
  scheduler whose tasks interleave on one thread — one of which fails without
  taking the others with it. The playground gains three coroutine programs
  alongside it.
- **The examples are tiered, and there are ten more of them.** Both the
  playground picker and the Examples index group their programs as *Simple*,
  *Intermediate* and *Advanced* — by the features you need to know to read
  one, not by length — and each example page carries its tier as a sidebar
  badge. The playground gains strings and text, tables, loops and control
  flow, functions (defaults, named arguments, multiple returns, variadics),
  error handling, enums, custom iterables, bitwise operators, wrapper types
  and `when(…)` pipelines; its null-safety, pattern-matching and generics
  programs were rewritten to cover `?.`/`?[`/`!`, guards and bindings, and
  bounds. All 24 run under both engines in `npm run check-samples`.

### Changed

- **Every benchmark is now under 2x Lua; the suite is at 1.33x.** These
  changes took the four that were still above it below:
  - **`closure`** (2.5x → **0.8x**): a local that holds one lambda for its
    whole life, captures nothing and is never reassigned is called as its
    proto (`CALLK`), not as a value. If the body is arithmetic or
    comparisons the typechecker proved numeric — which can neither fail nor
    call anything — it is compiled at the call site instead.
  - **`match`** (2.5x → **1.7x**): a payload field is one `GETPAY`,
    replacing `UNWRAP`, `LOADI`, `GETARR` and a `MOVE`. A guard is a fused
    compare-and-jump, not a boolean tested afterwards. A local scrutinee is
    matched where it is. A chain whose arms are all variants of one enum
    jumps on the tag to its first candidate arm instead of testing the arms
    above it one by one.
  - **`fib`** (2.1x → **1.8x**): a static method calling itself is a
    `CALLSELF`, which looks nothing up. A recursive call and its return stay
    in the dispatch loop instead of re-deriving the frame. `if n < 2 then
    return n end` is one inverted test instead of a test and a jump. The
    common one-value return skips `pop_frame`.
  - **`bytes`** (2.2x → **1.6x**): a `byte` is an integer at run time, so
    arithmetic on one now compiles to the integer opcodes instead of the
    dynamic `ARITHX`.

  Across the suite, a dynamic call no longer counts a reference to the
  closure it calls, a static call reaches its callee through one cache cell
  instead of three tables, and the dispatch loop decodes an opcode without a
  table load.
- **A chain of field reads is one instruction.** `row.mid?.inner?.v`,
  `self.body.pos.x` and `a.b!.c!` each compiled to a field read per link, and
  every `?.` added a four-instruction nil test around its read. Each read also
  copied the object in the middle of the chain into a register — a reference
  count bumped and dropped for an object the program was only passing
  through. The compiler now records the chain's field slots once, as data in
  the function, and one `GETPATH` walks them in place, copying only the value
  at the end. Errors still point at the link that failed. With it, `x ?? 0`
  and `x or ""` skip a literal fallback with one test instead of jumping
  around it, `-1` is loaded as a literal rather than negated at run time,
  and `t?[i]` on a table is a nil test and one read. `nullsafe` −35%
  (2.7x Lua → **1.7x**).
- **Building an enum variant with a payload allocates three times, not
  seven.** `Event.Click(x, y)` rebuilt both names — the enum's and the
  variant's — from scratch on every construction: a `String` out of the enum
  table and a `SauleStr` box around it, twice, for text that never changes.
  The names are now made once per engine and shared. `match` −11%
  (2.9x Lua → **2.5x**).
- **A pattern search stops zeroing 512 bytes per character.** `find` built a
  fresh match state at every start position it tried, and building one
  clears its 32-entry capture array; an unanchored search over a line did
  that once per character. The state is now reset instead — nothing reads a
  capture above the current level, so it is the same state. `String.gsub`
  also writes each replacement straight into its result, reading `%1`…`%9`
  by span rather than building every capture's text to use one, and `match`
  and `gsub` decode their arguments into reused buffers. `pattern` −17%
  (2.1x Lua → **1.75x**), `wordfreq` −4%.
- **`t[i]` with an integer inside the array is read and written in place.**
  Every indexed read and write called the table's general `get` or `set` out
  of line — which test every kind of key, string and hash part included — and
  wrote the result back with the full drop of whatever the register held,
  even a float. An integer key inside the array part now goes straight to the
  slot, and anything else to the general path, so the answer is the same
  either way; a store still tells the cycle collector about a container, and
  a test now says so. `matrix` −21% (2.2x Lua → **1.8x**), `array` −5%.
- **Copying a value is one branch, not a jump table — 14% across the suite.**
  `Value::clone` matched `Nil`, `Bool`, `Int` and `Float` as four arms, and
  LLVM lowered that to a jump table on the value's type. It is inlined into
  every `MOVE`, `GETF` and `LOADK` — a third of all instructions executed —
  so each of those carried a second indirect branch after the dispatch's own,
  predicted on the type of whatever was being copied. Copying any of the four
  is the same sixteen bytes, so one "is it a scalar?" test now picks the
  path. Found by reading the dispatch loop's machine code rather than its
  profile, which only ever showed the cost smeared across `MOVE`. The suite's
  geomean against Lua goes from 1.72x to **1.52x**; `oop` −25%, `closure`
  −23%, `entity` −20%, `bytes` −18%, `fib` −13%, and nothing slower.
- **A static call stops counting its closure, and a return of one value takes
  a straight path.** A call to a statically-resolved function (`CALLK`,
  `CALLSTAT` and their tail forms) took its closure out of the engine's cache
  and bumped its reference count, and the return dropped it again — on the
  same counter, every call, for a recursive function. The cache owns that
  closure for the life of the engine, which is the guarantee that already lets
  a frame hold its proto and chunk uncounted, so the frame now holds this one
  the same way; a closure taken from a register is still owned, for the
  reason the self-reassigning-closure test pins. And `RET1` into a caller
  that wants one value — nearly every return — skips the general result loop,
  its stack-growth check and its bounds checks, and the drop of an integer it
  overwrites. `fib` −15% (2.4x Lua → **2.1x**), `bintree`, `errors`, `json`
  −2% to −4%.
- **`Iter.unique` looks plain values up in a hash map.** It compared each
  element against everything kept so far, and cloned both operands of every
  comparison into the general `==` on top: about 50,000 comparisons and
  100,000 clones per call on 1000 integers with 97 distinct values, where the
  Lua twin makes one table lookup per element. Plain values — everything but
  objects — now go through a hash key built to agree exactly with the
  language's `==`: `0.0` and `-0.0` are one value, `NaN` is never a
  duplicate, `1`, `1.0` and `"1"` stay three. An object with an `OpEq`
  overload is still asked, and asked in the same order and the same number of
  times as before, so an overload with a side effect behaves as it did.
  `iterlib` −47%, 3.5x Lua → **2.0x**.
- **`Table.sort` moves elements instead of copying them.** Each merge pass
  copied every element into a scratch buffer and copied the run back over the
  original — two clones and a drop per element per pass, on each of the ~18
  passes a 200k-element sort makes. The merge now moves values between two
  buffers that swap roles every pass. `sort` −13%, `tablelib` −8%.
- **A method call on a generic class is a vtable call.** On a receiver typed
  `Stack<integer>` the compiler could prove no class at all — it recognised a
  receiver's type only when the type was a bare name — so every `st.push(…)`
  compiled to `CALLMX`, a by-name lookup and a re-entrant call, and every
  `st.top` to `GETFX`. Type arguments are erased before classes are laid out,
  so the head name *is* the layout: `Stack<integer>` now dispatches exactly
  like a non-generic class, and a generic interface receiver through its
  itable. `generic` −37%, 3.8x Lua → **2.4x**.
- **A call site that sees a few classes remembers all of them.** The cache
  behind `CALLM` and `CALLIF` held one class, so a site visiting two or three
  in turn missed every time — and *rewrote* itself every time, an `Rc` clone,
  a `borrow_mut` and a drop of the class it evicted, on top of the full
  lookup. `iface` rotates through three shapes, so its one hot site missed on
  100% of calls and the cache cost 20% of the program, more than having none.
  A site now widens from one class to four, and past four stops writing
  altogether. `iface` −23% (2.5x → **1.9x**), `entity` −14% (2.2x → 1.9x);
  sites that only ever see one class are unchanged.
- **A native that calls back a million times sets up once, not a million
  times.** `Table.sort`'s comparator and every `Iter` combinator's callback
  run on a register file of their own — the dispatch loop holds `&mut Vm`
  across the native that is calling back, so the callee cannot have the
  caller's — and *getting* one was per call what it could be per sort:
  upgrading a `Weak`, taking a file out of the re-entry pool, handing it back
  (which resets every register the callback touched), recovering the callee's
  proto by downcast, and taking a call-depth guard. A `CallSession` holds all
  of that open for the run of calls — the depth guard included, since the
  calls are sequential and one guard bounds the same nesting — and the `Iter`
  combinators stop allocating a fresh argument `Vec` per element on top of
  it. `sort` −17% (2.9x Lua → **2.4x**), `iterlib` −17% (4.2x → **3.5x**),
  `tablelib` −9%; the suite's geomean 1.9x → **1.8x**. What is left is no
  longer the outlier it was: the same lambda costs 34 ns called from Saule
  and 8.4 ns called from Lua, so the *ordinary* call path is now the thing to
  attack.
- **A program that does nothing starts in half the time.** `saule run` on an
  empty file took 2.80 ms, and 1.50 ms of that was reading every installed
  native package's library off the disk — a 1.8 MB library costs 0.8 ms in the
  read and another 0.75 ms parsing the 114 metadata records inside it — to
  learn names that most programs never ask for, before the program's first
  byte was lexed. The scan now happens at the first `import` or other question
  that needs it, so a program that imports no package never touches the
  packages directory: **2.80 ms → 1.34 ms**, against Lua's 0.75 ms on the same
  machine, and the `startup` benchmark from 3.9x Lua to 1.9x. Nothing about
  what resolves, or what an unusable package reports, changed. The one reader
  that used to force the scan on every run was the typechecker's signature
  table — it registers what has been discovered and no longer does the
  discovering, which import resolution, running first and asking per `import`,
  already did.
- **A call no longer touches three reference counts.** A `Frame` owned its
  closure, its `Proto` and its `Chunk`, so entering and leaving one — which
  happens on every call *and* every return — cost three increments and three
  decrements. Two were for a guarantee the engine already makes: a `Chunk`
  owns every `Proto` the VM runs and `VmShared` owns every `Chunk`, both for
  longer than any frame over them, which is the invariant the dispatch loop
  already relies on to *read* them out of a frame without a clone. The frame
  now carries them the same way, so only the closure handle is counted — and
  that asymmetry is deliberate: a closure's last strong reference can be the
  very register or upvalue cell its own body overwrites. Worth `fib` −23%,
  `closure` −12%, `bintree` −12%, `errors` −11%, `vecops` −10%, and −6% to
  −8% on `sort`, `generic`, `match`, `iface` and `oop`; the core-mechanisms
  geomean goes from 1.8x Lua to 1.7x and the suite's from 2.0x to 1.9x.
  Call-free code is untouched, as it should be: `mandel`, `strings`,
  `nullsafe` and `mathlib` do not move.
- **`obj.field!` is one instruction, and `x != nil` is two.** Two codegen
  fusions the VM's opcode table already had the opcodes for but nothing
  emitted. A force-unwrapped field read was `GETF` + `UNWRAPNIL`, which
  cloned the value into a temporary and then out of it again — a
  reference-count round trip per link — and is now a single `GETFU`; a
  nil comparison fell past every fused branch form and materialised a
  boolean through `EQV` + `NOT` + `TEST`, and is now `JNIL` / `JNOTNIL`.
  Both were measured with `--profile-bytecode` first, per `VM_DESIGN.md`
  §16: `GETF UNWRAPNIL` was the top adjacent pair in
  `benchmarks/sau/nullsafe.sau` at 5.7%, and the `EQV`/`NOT`/`TEST` triple
  about an eighth of the same program. Together they take 18% of its
  instructions out and `nullsafe` from 3.3x Lua to 2.9x; object-graph
  walking such as `bintree` gains a little too. **The copy is what costs,
  not the dispatch** — removing the cheaper triple alone moved the wall
  clock not at all, which is worth knowing before fusing the next pair.
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

- **A program's own class or enum is not mistaken for a native package's.**
  Discovering a native package puts its static classes' members in a table
  the checker shares across every module, and that table was consulted
  before the module's own declarations. So a module that declared `enum
  Event` — UIKit does — and imported anything at all from a package with an
  `Event` class had `Event.KeyPressed` reported as missing from the
  package's class. The module's own declaration now answers first; a typo on
  the package's class is still caught.
- **An `as` in a field default converts.** `static f: float = 230 as float`
  left the field `nil`: the checker compared a field's default against its
  declared type but never walked the expression, so the `as` was never told
  it was a conversion and ran as the type test instead ("is 230 a float?").
  Static and instance defaults alike. Walking the default also means an error
  inside one is now reported, where before it went unnoticed.
- **`f()?[i]` calls `f` once.** A safe index compiled its receiver twice — once
  for the nil test and again for the read — so a call there ran twice, side
  effects and all.
- **An index assignment on a generic class calls its `OpNewIndex`.**
  `grid[1] = v` on a `Grid<T>` failed at run time with an internal "typed
  opcode expected `table` but the register held `instance`": the compiler,
  unable to prove a class for a generic receiver, took the assignment for a
  table write. It is the class's overload now, as it already was on a
  non-generic class.
- **An `Iter` combinator reports a callback that is not a function even when
  the source is empty.** The check sat inside the per-element loop, so
  `Iter.map({}, 42)` quietly answered `{}` — the one input where a program
  could pass a number as its callback and hear nothing about it. It is made
  once, before the loop.
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
