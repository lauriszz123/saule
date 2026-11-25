//! `type` aliases across files.
//!
//! Inside one module an alias is a rewrite of that module's own types, and
//! the fixtures under `tests/run/aliases/` cover it. What only a program of
//! several files can show is the other half: an alias is written out *in
//! the module that declares it*, so what an importer receives — a class's
//! fields, a function's parameters, the alias itself — already says the
//! real type, and none of the declaring module's names leak across.
//!
//! Each test writes a small project, compiles it as `saule run` would, and
//! reads what it printed.

use std::path::{Path, PathBuf};

/// Write a throwaway project and hand back its directory.
fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("type_aliases")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create project dir");
    for (file, body) in files {
        let path = dir.join(file);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create subdir");
        }
        std::fs::write(&path, body).expect("write module");
    }
    dir
}

fn compile(entry: &Path) -> Result<saule_vm::program::Program, String> {
    saule_runtime::init();
    saule_vm::program::compile(entry).map_err(|e| render(&e))
}

/// A program error with everything it says: the message, and the message
/// of the diagnostic it wraps when the fault is in an imported module.
fn render(e: &saule_vm::program::ProgramError) -> String {
    format!("{e} | {e:?}")
}

/// Compile and run the project's `main.sau`, returning what it printed.
fn run(name: &str, files: &[(&str, &str)]) -> String {
    let dir = project(name, files);
    let program = match compile(&dir.join("main.sau")) {
        Ok(p) => p,
        Err(e) => panic!("expected `{name}` to compile: {e}"),
    };
    let (sink, ()) = saule_runtime::output::capture(|| {
        saule_vm::run_program(program).expect("the program must run");
    });
    sink.text().trim().to_string()
}

/// The error compiling the project's `main.sau`.
fn refused(name: &str, files: &[(&str, &str)]) -> String {
    let dir = project(name, files);
    match compile(&dir.join("main.sau")) {
        Ok(_) => panic!("expected `{name}` to be refused"),
        Err(e) => e,
    }
}

const EVENTS: &str = "\
export class Event
  name: string
  fn init(name: string)
    self.name = name
  end
end

-- Private: no other module can name this.
type Handler = fn(Event) -> nil

export type Listener = Handler
export type Listeners = table<string, table<Listener>>
export type Maybe<T> = T?
export type Count = integer

export class Bus
  listeners: Listeners = {}

  fn on(name: string, handler: Handler) -> nil
    local list = self.listeners[name] ?? {}
    Table.insert(list, handler)
    self.listeners[name] = list
  end

  fn emit(event: Event) -> Count
    local list = self.listeners[event.name] ?? {}
    for handler in list do
      handler(event)
    end
    return #list
  end
end

export fn first(listeners: Listeners, name: string) -> Maybe<Listener>
  return listeners[name]?[1]
end
";

#[test]
fn an_exported_alias_is_imported_by_name() {
    let out = run(
        "by_name",
        &[
            ("events.sau", EVENTS),
            (
                "main.sau",
                "from events import Bus, Event, Listener, Count\n\
                 local seen: table<string> = {}\n\
                 local listener: Listener = fn(e: Event) -> nil\n\
                 \x20 Table.insert(seen, e.name)\n\
                 end\n\
                 local bus = Bus()\n\
                 bus.on(\"tap\", listener)\n\
                 local n: Count = bus.emit(Event(\"tap\"))\n\
                 local plain: integer = n\n\
                 println(plain, String.join(\",\", seen))\n",
            ),
        ],
    );
    assert_eq!(out, "1\ttap");
}

/// `Bus.on` takes a `Handler`, an alias only `events.sau` knows. The
/// importer is checked against what it stands for, and may have a
/// `Handler` of its own meaning something else entirely.
#[test]
fn a_private_alias_does_not_leak_into_the_importer() {
    let out = run(
        "private",
        &[
            ("events.sau", EVENTS),
            (
                "main.sau",
                "from events import Bus, Event\n\
                 type Handler = fn(string) -> string\n\
                 local shout: Handler = fn(text: string) -> string\n\
                 \x20 return String.upper(text)\n\
                 end\n\
                 local heard = \"\"\n\
                 local bus = Bus()\n\
                 bus.on(\"tap\", fn(e: Event) -> nil\n\
                 \x20 heard = shout(e.name)\n\
                 end)\n\
                 bus.emit(Event(\"tap\"))\n\
                 println(heard)\n",
            ),
        ],
    );
    assert_eq!(out, "TAP");
}

#[test]
fn the_importers_own_alias_is_not_what_the_imported_class_means() {
    // `shout` is a `Handler` here, and not what `Bus.on` asks for.
    let err = refused(
        "private_mismatch",
        &[
            ("events.sau", EVENTS),
            (
                "main.sau",
                "from events import Bus\n\
                 type Handler = fn(string) -> string\n\
                 local shout: Handler = fn(text: string) -> string\n\
                 \x20 return text\n\
                 end\n\
                 Bus().on(\"tap\", shout)\n",
            ),
        ],
    );
    assert!(err.contains("fn(Event) -> nil"), "{err}");
}

#[test]
fn a_private_alias_cannot_be_imported() {
    let err = refused(
        "not_exported",
        &[
            ("events.sau", EVENTS),
            (
                "main.sau",
                "from events import Handler\nlocal h: Handler? = nil\nprintln(h)\n",
            ),
        ],
    );
    assert!(err.contains("Handler"), "{err}");
}

#[test]
fn an_alias_is_renamed_on_import() {
    let out = run(
        "renamed",
        &[
            ("events.sau", EVENTS),
            (
                "main.sau",
                "from events import Count as Tally, Maybe as Opt\n\
                 local n: Tally = 3\n\
                 local missing: Opt<string> = nil\n\
                 local present: Opt<Tally> = n + 1\n\
                 println(n, missing, present)\n",
            ),
        ],
    );
    assert_eq!(out, "3\tnil\t4");
}

#[test]
fn a_wildcard_import_brings_the_aliases() {
    let out = run(
        "wildcard",
        &[
            ("events.sau", EVENTS),
            (
                "main.sau",
                "from events import *\n\
                 local listeners: Listeners = {}\n\
                 local quiet: Listener = fn(e: Event) -> nil end\n\
                 listeners[\"tap\"] = {quiet}\n\
                 local found: Maybe<Listener> = first(listeners, \"tap\")\n\
                 local none = first(listeners, \"drag\")\n\
                 local n: Count = #listeners[\"tap\"]!\n\
                 println(found != nil, none == nil, n)\n",
            ),
        ],
    );
    assert_eq!(out, "true\ttrue\t1");
}

#[test]
fn a_namespace_import_reaches_an_alias_through_the_namespace() {
    let out = run(
        "namespace",
        &[
            ("events.sau", EVENTS),
            (
                "main.sau",
                "import events\n\
                 local n: events.Count = 2\n\
                 local all: table<events.Count> = {n, n + 1}\n\
                 local last: events.Maybe<integer> = all[2]\n\
                 println(n, #all, last)\n",
            ),
        ],
    );
    assert_eq!(out, "2\t2\t3");
}

#[test]
fn an_alias_is_passed_on_by_a_re_export() {
    let out = run(
        "re_export",
        &[
            ("events.sau", EVENTS),
            (
                "hub.sau",
                "export from events import Listener, Maybe as Opt\n\
                 export from events import Bus, Event\n",
            ),
            (
                "main.sau",
                "from hub import Bus, Event, Listener, Opt\n\
                 local count = 0\n\
                 local listener: Listener = fn(e: Event) -> nil\n\
                 \x20 count += 1\n\
                 end\n\
                 local bus = Bus()\n\
                 bus.on(\"a\", listener)\n\
                 bus.on(\"a\", listener)\n\
                 bus.emit(Event(\"a\"))\n\
                 local last: Opt<string> = nil\n\
                 println(count, last)\n",
            ),
        ],
    );
    assert_eq!(out, "2\tnil");
}

#[test]
fn an_alias_is_passed_on_by_a_wildcard_re_export() {
    let out = run(
        "re_export_all",
        &[
            ("kit/events.sau", EVENTS),
            ("kit/init.sau", "export from events import *\n"),
            (
                "main.sau",
                "from kit import *\n\
                 local n: Count = 5\n\
                 local l: Maybe<Listener> = nil\n\
                 println(n, l)\n",
            ),
        ],
    );
    assert_eq!(out, "5\tnil");
}

/// Three modules, each alias built on the one before. The last importer
/// sees none of the intermediate names and needs none of them.
#[test]
fn an_alias_is_built_on_an_imported_alias() {
    let out = run(
        "chain",
        &[
            ("a.sau", "export type Id = integer\n"),
            (
                "b.sau",
                "from a import Id\nexport type Ids = table<Id>\nexport type Index = table<string, Ids>\n",
            ),
            (
                "c.sau",
                "from b import Index, Ids\n\
                 export type Lookup = fn(Index, string) -> Ids?\n\
                 export fn lookup() -> Lookup\n\
                 \x20 return fn(index: Index, key: string) -> Ids?\n\
                 \x20   return index[key]\n\
                 \x20 end\n\
                 end\n",
            ),
            (
                "main.sau",
                "from c import Lookup, lookup\n\
                 local find: Lookup = lookup()\n\
                 local index: table<string, table<integer>> = {odd: {1, 3}}\n\
                 local odd: table<integer>? = find(index, \"odd\")\n\
                 println(#odd!, find(index, \"even\"))\n",
            ),
        ],
    );
    assert_eq!(out, "2\tnil");
}

/// An alias of a class reaches an importer that never imports the class:
/// as an annotation, behind `as`, and in a `catch`.
#[test]
fn an_alias_of_a_class_works_without_importing_the_class() {
    let out = run(
        "class_alias",
        &[
            (
                "geo.sau",
                "export class Point\n\
                 \x20 x: integer = 0\n\
                 \x20 fn init(x: integer)\n\
                 \x20   self.x = x\n\
                 \x20 end\n\
                 end\n\
                 export type Position = Point\n\
                 export type Path = table<Position>\n\
                 export fn at(x: integer) -> Position\n\
                 \x20 return Point(x)\n\
                 end\n",
            ),
            (
                "main.sau",
                "from geo import Position, Path, at\n\
                 local p: Position = at(4)\n\
                 local path: Path = {p, at(6)}\n\
                 local loose: any = p\n\
                 local back = loose as Position\n\
                 local not_one: any = 3\n\
                 local caught = 0\n\
                 try\n\
                 \x20 throw path[2]!\n\
                 catch e: Position\n\
                 \x20 caught = e.x\n\
                 end\n\
                 println(p.x, #path, back != nil, (not_one as Position) == nil, caught)\n",
            ),
        ],
    );
    assert_eq!(out, "4\t2\ttrue\ttrue\t6");
}

#[test]
fn a_generic_alias_is_checked_across_modules() {
    let err = refused(
        "generic_mismatch",
        &[
            ("events.sau", EVENTS),
            (
                "main.sau",
                "from events import Maybe\nlocal n: Maybe<integer> = \"three\"\nprintln(n)\n",
            ),
        ],
    );
    assert!(err.contains("integer"), "{err}");
}

#[test]
fn a_lambda_is_checked_against_an_imported_function_alias() {
    let err = refused(
        "lambda_mismatch",
        &[
            ("events.sau", EVENTS),
            (
                "main.sau",
                "from events import Listener\n\
                 local l: Listener = fn(text: string) -> nil end\n\
                 println(l)\n",
            ),
        ],
    );
    assert!(err.contains("fn(Event) -> nil"), "{err}");
}

/// What an alias stands for may name a type the importer has not imported.
/// The importer still knows it, as it knows the type of an imported
/// class's field.
#[test]
fn the_types_an_alias_mentions_are_known_to_the_importer() {
    let out = run(
        "mentioned",
        &[
            ("events.sau", EVENTS),
            (
                "main.sau",
                "from events import Listener\n\
                 local heard = \"\"\n\
                 local l: Listener = fn(e) -> nil\n\
                 \x20 heard = e.name\n\
                 end\n\
                 println(heard == \"\")\n",
            ),
        ],
    );
    assert_eq!(out, "true");
}

#[test]
fn an_imported_alias_is_not_a_value() {
    let err = refused(
        "as_value",
        &[
            ("events.sau", EVENTS),
            (
                "main.sau",
                "from events import Listener\nlocal l = Listener\nprintln(l)\n",
            ),
        ],
    );
    assert!(err.contains("is a type alias, not a value"), "{err}");
}

/// The first import to bind a name keeps it, whatever kind of thing the
/// later one has by that name — the same rule the linker follows.
#[test]
fn the_first_import_of_a_name_wins_between_an_alias_and_a_class() {
    let files_a = ("a.sau", "export type Thing = integer\n");
    let files_b = (
        "b.sau",
        "export class Thing\n  label: string = \"class\"\nend\n",
    );
    let alias_first = run(
        "first_wins_alias",
        &[
            files_a,
            files_b,
            (
                "main.sau",
                "from a import Thing\nfrom b import *\nlocal t: Thing = 3\nprintln(t + 1)\n",
            ),
        ],
    );
    assert_eq!(alias_first, "4");

    let class_first = run(
        "first_wins_class",
        &[
            files_a,
            files_b,
            (
                "main.sau",
                "from b import Thing\nfrom a import *\nlocal t: Thing = Thing()\nprintln(t.label)\n",
            ),
        ],
    );
    assert_eq!(class_first, "class");
}

/// A module's own declaration wins over anything imported under its name.
#[test]
fn a_declared_alias_wins_over_an_imported_one() {
    let out = run(
        "own_wins",
        &[
            ("events.sau", EVENTS),
            (
                "main.sau",
                "from events import *\n\
                 type Count = string\n\
                 local c: Count = \"many\"\n\
                 println(c)\n",
            ),
        ],
    );
    assert_eq!(out, "many");
}

/// A mistake in an imported module's alias is that module's to report.
#[test]
fn a_broken_alias_is_reported_in_the_module_that_declares_it() {
    let err = refused(
        "broken_import",
        &[
            (
                "broken.sau",
                "export type Loop = table<Loop>\nexport fn f() -> integer\n  return 1\nend\n",
            ),
            ("main.sau", "from broken import f\nprintln(f())\n"),
        ],
    );
    assert!(err.contains("defined in terms of itself"), "{err}");
    assert!(err.contains("broken.sau"), "{err}");
}

/// Nothing of an alias exists at run time: a module that only declares
/// aliases and a module that imports them compile to no code for them.
#[test]
fn an_alias_leaves_nothing_behind_at_run_time() {
    let dir = project(
        "nothing_left",
        &[
            (
                "types.sau",
                "export type Id = integer\nexport type Ids = table<Id>\n",
            ),
            (
                "main.sau",
                "from types import Id, Ids\nlocal ids: Ids = {1, 2}\nlocal id: Id = ids[1]!\nprintln(id)\n",
            ),
        ],
    );
    let with_aliases = compile(&dir.join("main.sau")).expect("compiles");

    let plain = project(
        "nothing_left_plain",
        &[
            ("types.sau", "\n"),
            (
                "main.sau",
                "from types import *\nlocal ids: table<integer> = {1, 2}\nlocal id: integer = ids[1]!\nprintln(id)\n",
            ),
        ],
    );
    let without = compile(&plain.join("main.sau")).expect("compiles");

    let code = |p: &saule_vm::program::Program| -> Vec<usize> {
        p.modules
            .iter()
            .map(|m| m.protos.iter().map(|proto| proto.code.len()).sum())
            .collect()
    };
    assert_eq!(code(&with_aliases), code(&without));
}
