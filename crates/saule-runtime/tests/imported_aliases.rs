//! The import seed and `type` aliases.
//!
//! What a module's imports bring in is read from the imported files without
//! running them (`collect_import_seed`). For aliases that has two halves,
//! and these pin both at the level the checker, the compiler and the
//! language server all share:
//!
//! * the aliases themselves arrive in `ModuleSeed::aliases`, written out in
//!   full, under the names the import gives them;
//! * everything else an imported module declares — a class's fields, a
//!   function's signature — arrives with that module's aliases already
//!   replaced, so no name of the imported module's has to mean anything in
//!   the importing one.

use std::path::{Path, PathBuf};

use saule_ast::{Module, Type};
use saule_runtime::module::{SeedIo, collect_import_seed, collect_import_seed_io};
use saule_semantic::ModuleSeed;

fn parse(src: &str) -> Module {
    let tokens = saule_lexer::Lexer::new(src).tokenize().expect("lex");
    saule_parser::parse(tokens).expect("parse")
}

/// Fresh scratch directory, unique per test so the suite can run in
/// parallel.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("saule-impalias-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

fn write(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create a directory");
    }
    std::fs::write(path, body).expect("write a module");
}

fn seed_of(src: &str, dir: &Path) -> ModuleSeed {
    saule_runtime::init();
    collect_import_seed(&parse(src), dir)
}

fn show(ty: &Type) -> String {
    match ty {
        Type::Named(n) => n.clone(),
        Type::Nullable(inner) => format!("{}?", show(inner)),
        Type::Table { key: None, value } => format!("table<{}>", show(value)),
        Type::Table {
            key: Some(k),
            value,
        } => format!("table<{}, {}>", show(k), show(value)),
        Type::Tuple(items) => format!(
            "({})",
            items.iter().map(show).collect::<Vec<_>>().join(", ")
        ),
        Type::Function { params, ret } => format!(
            "fn({}) -> {}",
            params.iter().map(show).collect::<Vec<_>>().join(", "),
            show(ret)
        ),
        Type::Generic(g) => format!(
            "{}<{}>",
            g.name,
            g.args.iter().map(show).collect::<Vec<_>>().join(", ")
        ),
    }
}

/// The aliases in `seed`, as sorted `name = type` lines.
fn aliases(seed: &ModuleSeed) -> Vec<String> {
    let mut out: Vec<String> = seed
        .aliases
        .iter()
        .map(|(name, info)| {
            let params = if info.type_params.is_empty() {
                String::new()
            } else {
                format!("<{}>", info.type_params.join(", "))
            };
            format!("{name}{params} = {}", show(&info.ty))
        })
        .collect();
    out.sort();
    out
}

/// Type errors of `src`, checked as a file in `dir`.
fn type_errors(src: &str, dir: &Path) -> Vec<String> {
    saule_runtime::init();
    let module = parse(src);
    let seed = collect_import_seed(&module, dir);
    let semantic = saule_semantic::analyze_with_seed(&module, seed);
    let mut out: Vec<String> = semantic.iter().map(|e| e.to_string()).collect();
    out.extend(saule_typeck::check(&module).iter().map(|e| e.to_string()));
    out
}

const SHAPES: &str = "\
type Coord = float
type Private = table<Coord>

export type Point = table<Coord>
export type Path = table<Point>
export type Visit<T> = fn(Point, T) -> T
export type Maybe<T> = T?

export class Canvas
  points: Path = {}
  origin: Private = {0.0, 0.0}

  fn add(point: Point) -> Canvas
    Table.insert(self.points, point)
    return self
  end

  fn fold<T>(start: T, visit: Visit<T>) -> T
    local acc = start
    for point in self.points do
      acc = visit(point, acc)
    end
    return acc
  end
end

export fn first(path: Path) -> Maybe<Point>
  return path[1]
end

export limit: Coord = 100.0
";

#[test]
fn exported_aliases_arrive_written_out() {
    let dir = scratch("exported");
    write(&dir, "shapes.sau", SHAPES);
    let seed = seed_of("from shapes import Point, Path, Visit, Maybe\n", &dir);
    assert_eq!(
        aliases(&seed),
        [
            "Maybe<T> = T?",
            "Path = table<table<float>>",
            "Point = table<float>",
            "Visit<T> = fn(table<float>, T) -> T",
        ]
    );
}

#[test]
fn only_the_names_asked_for_arrive() {
    let dir = scratch("named");
    write(&dir, "shapes.sau", SHAPES);
    let seed = seed_of("from shapes import Point\n", &dir);
    assert_eq!(aliases(&seed), ["Point = table<float>"]);
}

#[test]
fn a_private_alias_never_arrives() {
    let dir = scratch("private");
    write(&dir, "shapes.sau", SHAPES);
    let seed = seed_of("from shapes import *\n", &dir);
    let names: Vec<&String> = seed.aliases.keys().collect();
    assert!(
        !names.iter().any(|n| *n == "Coord" || *n == "Private"),
        "{names:?}"
    );
    assert_eq!(seed.aliases.len(), 4);
}

#[test]
fn an_alias_arrives_under_the_name_the_import_gives_it() {
    let dir = scratch("renamed");
    write(&dir, "shapes.sau", SHAPES);
    let seed = seed_of("from shapes import Point as Vertex, Maybe as Opt\n", &dir);
    assert_eq!(aliases(&seed), ["Opt<T> = T?", "Vertex = table<float>"]);

    let seed = seed_of("import shapes\n", &dir);
    assert!(
        seed.aliases.contains_key("shapes.Point"),
        "{:?}",
        aliases(&seed)
    );
}

/// The other half: nothing else in the seed still says an alias's name.
#[test]
fn everything_else_arrives_with_the_aliases_replaced() {
    let dir = scratch("replaced");
    write(&dir, "shapes.sau", SHAPES);
    let seed = seed_of("from shapes import Canvas, first, limit\n", &dir);

    let canvas = &seed.classes["Canvas"];
    assert_eq!(show(&canvas.field_types["points"]), "table<table<float>>");
    // A field typed with an alias the module does not even export.
    assert_eq!(show(&canvas.field_types["origin"]), "table<float>");
    assert_eq!(show(&canvas.methods["add"].params[0].ty), "table<float>");
    // A generic alias applied to the method's own type parameter.
    let fold = &canvas.methods["fold"];
    assert_eq!(show(&fold.params[1].ty), "fn(table<float>, T) -> T");

    let first = &seed.functions["first"];
    assert_eq!(show(&first.params[0].ty), "table<table<float>>");
    assert_eq!(
        first.return_ty.as_ref().map(show).as_deref(),
        Some("table<float>?")
    );

    assert_eq!(show(&seed.variables["limit"]), "float");
    assert!(seed.aliases.is_empty(), "none was imported by name");
}

#[test]
fn an_importer_is_checked_against_what_the_alias_stands_for() {
    let dir = scratch("checked");
    write(&dir, "shapes.sau", SHAPES);
    let ok = "from shapes import Canvas, Point, first\n\
              local corner: Point = {1.0, 2.0}\n\
              local canvas = Canvas().add(corner)\n\
              local total: float = canvas.fold(0.0, fn(p: Point, acc: float) -> float\n\
              \x20 return acc + p[1]!\n\
              end)\n\
              local head: table<float>? = first(canvas.points)\n";
    assert_eq!(type_errors(ok, &dir), Vec::<String>::new());

    let wrong = "from shapes import Canvas\nlocal canvas = Canvas().add({\"a\", \"b\"})\n";
    let errs = type_errors(wrong, &dir);
    assert!(errs.iter().any(|e| e.contains("float")), "{errs:?}");
}

/// The importer's own `Coord` is its own; the class's fields were settled
/// where the class was declared.
#[test]
fn an_importers_alias_does_not_change_an_imported_classs_types() {
    let dir = scratch("own_name");
    write(&dir, "shapes.sau", SHAPES);
    let src = "from shapes import Canvas\n\
               type Coord = string\n\
               type Private = integer\n\
               local c = Canvas()\n\
               local x: float = c.origin[1]!\n\
               local label: Coord = \"north\"\n";
    assert_eq!(type_errors(src, &dir), Vec::<String>::new());
}

#[test]
fn aliases_pass_through_re_exports() {
    let dir = scratch("reexport");
    write(&dir, "shapes.sau", SHAPES);
    write(
        &dir,
        "kit.sau",
        "export from shapes import Point, Maybe as Opt\n",
    );
    write(
        &dir,
        "all.sau",
        "export from kit import *\nexport type Count = integer\n",
    );

    let seed = seed_of("from kit import *\n", &dir);
    assert_eq!(aliases(&seed), ["Opt<T> = T?", "Point = table<float>"]);

    let seed = seed_of("from all import *\n", &dir);
    assert_eq!(
        aliases(&seed),
        ["Count = integer", "Opt<T> = T?", "Point = table<float>"]
    );
}

/// An import that is not a re-export passes nothing on.
#[test]
fn a_plain_import_is_not_passed_on() {
    let dir = scratch("not_passed");
    write(&dir, "shapes.sau", SHAPES);
    write(
        &dir,
        "user.sau",
        "from shapes import Point\nexport type Line = table<Point>\n",
    );
    let seed = seed_of("from user import *\n", &dir);
    assert_eq!(aliases(&seed), ["Line = table<table<float>>"]);
}

#[test]
fn an_alias_built_across_modules_is_written_out_at_each_step() {
    let dir = scratch("chain");
    write(&dir, "a.sau", "export type Id = integer\n");
    write(
        &dir,
        "b.sau",
        "from a import Id\nexport type Ids = table<Id>\n",
    );
    write(
        &dir,
        "c.sau",
        "from b import Ids\nfrom a import Id as Key\nexport type Index = table<Key, Ids>\n",
    );
    let seed = seed_of("from c import Index\n", &dir);
    assert_eq!(aliases(&seed), ["Index = table<integer, table<integer>>"]);
}

/// The types an alias mentions are made known to the importer, as the
/// types of an imported class's fields are.
#[test]
fn a_class_an_alias_mentions_is_reachable() {
    let dir = scratch("reachable");
    write(
        &dir,
        "events.sau",
        "export class Event\n  name: string = \"\"\nend\nexport type Handler = fn(Event) -> nil\n",
    );
    let src = "from events import Handler\n\
               local seen: string = \"\"\n\
               local h: Handler = fn(e) -> nil\n\
               \x20 seen = e.name\n\
               end\n";
    let seed = seed_of(src, &dir);
    assert!(
        seed.classes.contains_key("Event"),
        "the mentioned class is seeded"
    );
    assert_eq!(type_errors(src, &dir), Vec::<String>::new());
}

/// Two modules that import each other. Neither can be finished before the
/// other, so the one met again is read with its own aliases only — and the
/// walk ends.
#[test]
fn an_import_cycle_ends() {
    let dir = scratch("cycle");
    write(
        &dir,
        "a.sau",
        "from b import B\nexport type A = table<integer>\n",
    );
    write(
        &dir,
        "b.sau",
        "from a import A\nexport type B = table<string>\n",
    );
    let seed = seed_of("from a import A\nfrom b import B\n", &dir);
    assert_eq!(aliases(&seed), ["A = table<integer>", "B = table<string>"]);
}

/// A module reached along many paths is worked out once per collection.
/// Thirty layers of two imports each is a billion paths; this returns.
#[test]
fn a_module_reached_along_many_paths_is_resolved_once() {
    let dir = scratch("diamond");
    write(&dir, "l0.sau", "export type T0 = integer\n");
    for i in 1..=30 {
        let prev = i - 1;
        write(
            &dir,
            &format!("l{i}.sau"),
            &format!(
                "from l{prev} import T{prev}\nfrom l{prev} import T{prev} as Again\n\
                 export type T{i} = table<T{prev}>\n"
            ),
        );
    }
    let started = std::time::Instant::now();
    let seed = seed_of("from l30 import T30\n", &dir);
    assert!(seed.aliases.contains_key("T30"));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "took {:?}",
        started.elapsed()
    );
}

/// The language server's case: the importer is re-checked against the text
/// in the editor, not the file on disk.
#[test]
fn an_unsaved_edit_to_an_alias_reaches_the_importer() {
    saule_runtime::init();
    let dir = scratch("overlay");
    write(&dir, "shapes.sau", SHAPES);
    let shapes = saule_runtime::module::resolve_import_path(&dir, "shapes").expect("resolves");
    let module = parse("from shapes import Point, Canvas\n");

    let before = collect_import_seed(&module, &dir);
    assert_eq!(aliases(&before), ["Point = table<float>"]);

    // The same file, with `Coord` now an integer — in the buffer only.
    let edited = SHAPES.replace("type Coord = float", "type Coord = integer");
    let overlay = |p: &Path| (p == shapes).then(|| edited.clone());
    let after = collect_import_seed_io(&module, &dir, SeedIo::from_overlay(&overlay));
    assert_eq!(aliases(&after), ["Point = table<integer>"]);
    // And so has everything declared in terms of it.
    assert_eq!(
        show(&after.classes["Canvas"].field_types["points"]),
        "table<table<integer>>"
    );
    assert!(
        !before.same_surface(&after),
        "an importer has to be checked again"
    );

    // Back to the file on disk: the first answer again.
    let again = collect_import_seed(&module, &dir);
    assert!(before.same_surface(&again));
}

/// An edit that does not touch what an importer sees leaves the seed the
/// same, so the importer is not checked again for it.
#[test]
fn an_edit_behind_an_alias_that_changes_nothing_is_not_a_change() {
    saule_runtime::init();
    let dir = scratch("same_surface");
    write(&dir, "shapes.sau", SHAPES);
    let shapes = saule_runtime::module::resolve_import_path(&dir, "shapes").expect("resolves");
    let module = parse("from shapes import *\n");
    let before = collect_import_seed(&module, &dir);

    // `Coord` renamed throughout: every type an importer sees is unchanged.
    let edited = SHAPES.replace("Coord", "Scalar");
    let overlay = |p: &Path| (p == shapes).then(|| edited.clone());
    let after = collect_import_seed_io(&module, &dir, SeedIo::from_overlay(&overlay));
    assert!(before.same_surface(&after));
}

/// A mistake in an imported module's aliases does not stop its importer
/// from being read: the alias in error stands for `any` there.
#[test]
fn a_broken_alias_in_an_import_does_not_break_the_seed() {
    let dir = scratch("broken");
    write(
        &dir,
        "broken.sau",
        "export type Loop = table<Loop>\nexport type Fine = integer\nexport class Holder\n  loop: Loop = {}\nend\n",
    );
    let seed = seed_of("from broken import *\n", &dir);
    assert_eq!(aliases(&seed), ["Fine = integer", "Loop = any"]);
    assert_eq!(show(&seed.classes["Holder"].field_types["loop"]), "any");
}
