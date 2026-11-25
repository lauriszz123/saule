//! Integration tests for `saule_semantic::analyze`.
//!
//! Semantic analysis is the pass between parsing and typechecking: it
//! resolves names, populates the class / interface / enum registries, and
//! enforces the rules that are about *structure* rather than types —
//! every field initialised, every path returning, `self` only inside a
//! method, `break` only inside a loop.
//!
//! Everything here drives the public [`analyze`] entry point over real
//! source text, so the tests describe the language rather than the
//! implementation and survive refactors of the internals.

use saule_ast::Module;
use saule_semantic::{SemanticError, analyze};

fn parse(src: &str) -> Module {
    let tokens = saule_lexer::Lexer::new(src)
        .tokenize()
        .unwrap_or_else(|e| panic!("source should lex: {e:?}\n{src}"));
    saule_parser::parse(tokens).unwrap_or_else(|e| panic!("source should parse: {e:?}\n{src}"))
}

/// Analyse `src` and return the errors.
fn errors(src: &str) -> Vec<SemanticError> {
    analyze(&parse(src))
}

/// Assert `src` analyses cleanly.
fn accepts(src: &str) {
    let errs = errors(src);
    assert!(
        errs.is_empty(),
        "expected no errors, got {errs:?}\n--- source ---\n{src}"
    );
}

/// Assert `src` produces at least one error whose rendering mentions
/// `needle`. Matching on the message keeps these readable while still
/// pinning the specific rule that fired.
fn rejects(src: &str, needle: &str) {
    let errs = errors(src);
    assert!(
        !errs.is_empty(),
        "expected an error mentioning {needle:?}, got none\n--- source ---\n{src}"
    );
    let joined = errs
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(
        joined.contains(needle),
        "expected an error mentioning {needle:?}, got: {joined}\n--- source ---\n{src}"
    );
}

// ─── name resolution ────────────────────────────────────────────────────

#[test]
fn undefined_name_is_reported() {
    rejects("fn main() -> nil\n  println(nope)\n  return\nend\n", "nope");
}

#[test]
fn locals_are_visible_after_declaration() {
    // No prelude here: `saule_semantic` learns names like `println` from
    // an embedder-installed provider, which only the interpreter sets.
    // Prelude resolution is covered by the `.sau` suite instead.
    accepts("fn f() -> integer\n  local x: integer = 1\n  return x\nend\n");
}

#[test]
fn parameters_are_in_scope_in_the_body() {
    accepts("fn f(a: integer) -> integer\n  return a\nend\n");
}

#[test]
fn a_local_does_not_escape_its_function() {
    rejects(
        "\
fn a() -> nil
  local secret: integer = 1
  return
end
fn b() -> nil
  println(secret)
  return
end
",
        "secret",
    );
}

#[test]
fn static_members_are_reachable_by_bare_name_inside_methods() {
    // Static fields, static methods and instance methods resolve
    // unqualified from inside any method of the same class.
    accepts(
        "\
class C
  static cap: integer = 10
  fn limit() -> integer
    return cap
  end
  fn again() -> integer
    return self.limit()
  end
end
",
    );
}

#[test]
fn instance_fields_are_not_reachable_by_bare_name() {
    // Instance *fields* are deliberately excluded from bare-name lookup —
    // they need `self.`. (The README's `Counter` example gets this wrong
    // and does not compile.)
    rejects(
        "\
class C
  count: integer = 0
  fn bump() -> integer
    return count + 1
  end
end
",
        "count",
    );
}

// ─── self / super ───────────────────────────────────────────────────────

#[test]
fn self_outside_a_class_is_rejected() {
    rejects("fn f() -> nil\n  println(self)\n  return\nend\n", "self");
}

#[test]
fn self_inside_a_method_is_fine() {
    accepts(
        "\
class C
  x: integer = 0
  fn get() -> integer
    return self.x
  end
end
",
    );
}

// ─── field initialisation ───────────────────────────────────────────────

#[test]
fn non_nullable_field_never_assigned_is_reported() {
    rejects(
        "\
class P
  name: string
  fn init()
  end
end
",
        "name",
    );
}

#[test]
fn field_with_a_default_is_initialised() {
    accepts("class P\n  name: string = \"anon\"\nend\n");
}

#[test]
fn field_assigned_in_init_is_initialised() {
    accepts(
        "\
class P
  name: string
  fn init(n: string)
    self.name = n
  end
end
",
    );
}

#[test]
fn nullable_field_needs_no_initialiser() {
    accepts("class P\n  nickname: string?\nend\n");
}

// ─── return paths ───────────────────────────────────────────────────────

#[test]
fn missing_return_on_some_path_is_reported() {
    rejects(
        "\
fn f(flag: boolean) -> integer
  if flag then
    return 1
  end
end
",
        "return",
    );
}

#[test]
fn return_on_every_branch_is_accepted() {
    accepts(
        "\
fn f(flag: boolean) -> integer
  if flag then
    return 1
  else
    return 2
  end
end
",
    );
}

// ─── loop control ───────────────────────────────────────────────────────

#[test]
fn break_outside_a_loop_is_rejected() {
    rejects("fn f() -> nil\n  break\n  return\nend\n", "loop");
}

#[test]
fn break_inside_a_loop_is_fine() {
    accepts(
        "\
fn f() -> nil
  while true do
    break
  end
  return
end
",
    );
}

// ─── parameter lists ────────────────────────────────────────────────────

#[test]
fn variadic_must_come_last() {
    rejects(
        "fn f(...rest: integer, tail: integer) -> nil\n  return\nend\n",
        "variadic",
    );
}

#[test]
fn only_one_variadic_parameter_is_allowed() {
    rejects(
        "fn f(...a: integer, ...b: integer) -> nil\n  return\nend\n",
        "variadic",
    );
}

// ─── declaration shapes that must keep analysing cleanly ────────────────

#[test]
fn inheritance_and_interfaces_analyse() {
    accepts(
        "\
interface Drawable
  fn draw() -> nil
end

class Shape
  sides: integer = 0
end

class Square extends Shape implements Drawable
  fn init()
    self.sides = 4
  end
  fn draw() -> nil
    return
  end
end
",
    );
}

#[test]
fn enums_with_variants_and_methods_analyse() {
    accepts(
        "\
enum Direction
  North
  South
  fn flip() -> Direction
    return self
  end
end
",
    );
}

// ── Compound assignment ──────────────────────────────────────────────────

#[test]
fn compound_assignment_requires_a_declared_target() {
    rejects("zzz += 1", "zzz");
    accepts("local n: integer = 0\nn += 1");
}

#[test]
fn compound_assignment_resolves_names_in_the_value() {
    rejects("local n: integer = 0\nn += nope", "nope");
}

#[test]
fn compound_assignment_does_not_initialise_a_field() {
    // `self.n += 1` reads `self.n` before writing it, so it cannot be what
    // brings the field into existence — the definite-initialisation check
    // must still fire.
    rejects(
        r#"
        class C
            n: integer
            fn init()
                self.n += 1
            end
        end
        "#,
        "n",
    );
    accepts(
        r#"
        class C
            n: integer
            fn init()
                self.n = 0
                self.n += 1
            end
        end
        "#,
    );
}

// ─── `type` aliases ─────────────────────────────────────────────────────

use saule_ast::{Decl, Stmt, Type};
use saule_semantic::{
    AliasInfo, AliasRegistry, ModuleSeed, alias_scope, analyze_with_seed, expand_aliases,
    exported_aliases,
};

/// A type as written, for comparing against.
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

/// The type `text` spells.
fn ty(text: &str) -> Type {
    let module = parse(&format!("local probe: {text} = nil\n"));
    match &module.stmts[0].value {
        Stmt::Local { ty: Some(t), .. } => t.clone(),
        other => panic!("expected an annotated local, got {other:?}"),
    }
}

/// The scope of `src` with nothing imported, and the errors building it.
fn scope_of(src: &str) -> (AliasRegistry, Vec<String>) {
    let (scope, errors) = alias_scope(&parse(src), &AliasRegistry::new());
    (scope, errors.iter().map(|e| e.to_string()).collect())
}

/// What alias `name` stands for in `src`, written out.
fn stands_for(src: &str, name: &str) -> String {
    let (scope, errors) = scope_of(src);
    assert!(errors.is_empty(), "unexpected errors: {errors:?}\n{src}");
    show(
        &scope
            .get(name)
            .unwrap_or_else(|| panic!("no alias `{name}` in {scope:?}"))
            .ty,
    )
}

/// `src` with its aliases written out: the annotation of the local `name`.
fn local_type(src: &str, name: &str) -> String {
    let mut module = parse(src);
    let (scope, _) = alias_scope(&module, &AliasRegistry::new());
    let errors = expand_aliases(&mut module, &scope);
    assert!(errors.is_empty(), "unexpected errors: {errors:?}\n{src}");
    for s in &module.stmts {
        if let Stmt::Local {
            name: n,
            ty: Some(t),
            ..
        } = &s.value
            && n == name
        {
            return show(t);
        }
    }
    panic!("no annotated local `{name}` in\n{src}");
}

#[test]
fn a_module_without_aliases_has_an_empty_scope() {
    let (scope, errors) = scope_of("class A\nend\nfn f() -> integer\n  return 1\nend\n");
    assert!(scope.is_empty());
    assert!(errors.is_empty());
}

#[test]
fn an_alias_stands_for_the_type_on_its_right() {
    let src = "type Id = integer\ntype Handler = fn(string) -> nil\ntype Names = table<string>\n";
    assert_eq!(stands_for(src, "Id"), "integer");
    assert_eq!(stands_for(src, "Handler"), "fn(string) -> nil");
    assert_eq!(stands_for(src, "Names"), "table<string>");
}

#[test]
fn an_alias_of_an_alias_is_written_out_in_full() {
    let src = "type Id = integer\ntype Ids = table<Id>\ntype Index = table<string, Ids>\n";
    assert_eq!(stands_for(src, "Ids"), "table<integer>");
    assert_eq!(stands_for(src, "Index"), "table<string, table<integer>>");
}

/// Declaration order means nothing at module level, for aliases as for
/// functions and classes.
#[test]
fn an_alias_may_mention_one_declared_below_it() {
    let src = "type Index = table<string, Ids>\ntype Ids = table<Id>\ntype Id = integer\n";
    assert_eq!(stands_for(src, "Index"), "table<string, table<integer>>");
}

#[test]
fn a_generic_alias_keeps_its_parameters() {
    let (scope, _) = scope_of("type Pair<A, B> = table<A, B>\n");
    let pair = &scope["Pair"];
    assert_eq!(pair.type_params, ["A", "B"]);
    assert_eq!(show(&pair.ty), "table<A, B>");
}

#[test]
fn a_generic_alias_is_applied_inside_another_alias() {
    let src = "\
type Pair<A, B> = table<A, B>
type Named<T> = Pair<string, T>
type Ages = Named<integer>
type Swap<X, Y> = Pair<Y, X>
";
    assert_eq!(stands_for(src, "Named"), "table<string, T>");
    assert_eq!(stands_for(src, "Ages"), "table<string, integer>");
    assert_eq!(stands_for(src, "Swap"), "table<Y, X>");
}

/// Arguments are put in all at once: `Pair<B, A>` is not `A → B` followed
/// by `B → A`, which would leave both the same.
#[test]
fn type_arguments_are_substituted_simultaneously() {
    let src = "type Pair<A, B> = table<A, B>\ntype Flip<A, B> = Pair<B, A>\n";
    assert_eq!(stands_for(src, "Flip"), "table<B, A>");
    assert_eq!(
        local_type(&format!("{src}local x: Flip<string, integer> = nil\n"), "x"),
        "table<integer, string>"
    );
}

/// An alias's own parameter is that parameter, even when another alias has
/// its name.
#[test]
fn an_alias_parameter_shadows_an_alias_of_the_same_name() {
    let src = "type T = string\ntype Box<T> = table<T>\ntype Words = Box<T>\n";
    assert_eq!(stands_for(src, "Box"), "table<T>");
    // Outside `Box`, `T` is the alias again.
    assert_eq!(stands_for(src, "Words"), "table<string>");
}

#[test]
fn nullable_of_a_nullable_alias_is_one_layer() {
    let src = "type Maybe = string?\ntype Opt<T> = T?\n";
    assert_eq!(local_type(&format!("{src}local a: Maybe? = nil\n"), "a"), "string?");
    assert_eq!(
        local_type(&format!("{src}local a: Opt<integer>? = nil\n"), "a"),
        "integer?"
    );
    assert_eq!(
        local_type(&format!("{src}local a: Opt<Maybe> = nil\n"), "a"),
        "string?"
    );
}

#[test]
fn every_use_in_a_module_is_written_out() {
    let src = "\
type Id = integer
type Handler = fn(Id) -> nil
local a: Id = 1
local b: table<Id> = {}
local c: Handler? = nil
local d: fn(Handler) -> table<string, Id> = nil
";
    assert_eq!(local_type(src, "a"), "integer");
    assert_eq!(local_type(src, "b"), "table<integer>");
    assert_eq!(local_type(src, "c"), "fn(integer) -> nil?");
    assert_eq!(
        local_type(src, "d"),
        "fn(fn(integer) -> nil) -> table<string, integer>"
    );
}

/// The declaration is rewritten too: what a module exports is the alias in
/// full, not a chain to follow.
#[test]
fn the_declaration_itself_is_written_out() {
    let mut module = parse("type Id = integer\ntype Ids = table<Id>\n");
    let (scope, _) = alias_scope(&module, &AliasRegistry::new());
    expand_aliases(&mut module, &scope);
    let Stmt::Decl(d) = &module.stmts[1].value else {
        panic!("expected a declaration");
    };
    let Decl::TypeAlias { ty, .. } = &d.value else {
        panic!("expected an alias");
    };
    assert_eq!(show(ty), "table<integer>");
}

/// Inside `class Map<Key, Value>`, `Key` is the class's parameter, whatever
/// alias the module also has by that name.
#[test]
fn a_type_parameter_in_scope_is_not_rewritten() {
    let src = "\
type Key = string
class Map<Key, Value>
  one: Key = nil
  fn get(k: Key) -> Value
    local inner: Key = k
    return nil
  end
end
fn pick<Key>(k: Key) -> Key
  return k
end
fn plain(k: Key) -> Key
  return k
end
";
    let mut module = parse(src);
    let (scope, _) = alias_scope(&module, &AliasRegistry::new());
    expand_aliases(&mut module, &scope);
    let mut class_field = None;
    let mut generic_param = None;
    let mut plain_param = None;
    for s in &module.stmts {
        let Stmt::Decl(d) = &s.value else { continue };
        match &d.value {
            Decl::Class { members, .. } => {
                for m in members {
                    if let saule_ast::ClassMember::Field { ty, .. } = &m.value {
                        class_field = Some(show(ty));
                    }
                }
            }
            Decl::Function { name, params, .. } if name == "pick" => {
                generic_param = Some(show(&params[0].ty));
            }
            Decl::Function { name, params, .. } if name == "plain" => {
                plain_param = Some(show(&params[0].ty));
            }
            _ => {}
        }
    }
    assert_eq!(class_field.as_deref(), Some("Key"), "the class's parameter");
    assert_eq!(generic_param.as_deref(), Some("Key"), "the function's parameter");
    assert_eq!(plain_param.as_deref(), Some("string"), "the alias");
}

fn imported(pairs: &[(&str, &[&str], &str)]) -> AliasRegistry {
    pairs
        .iter()
        .map(|(name, params, text)| {
            (
                name.to_string(),
                AliasInfo {
                    type_params: params.iter().map(|p| p.to_string()).collect(),
                    ty: ty(text),
                },
            )
        })
        .collect()
}

#[test]
fn an_imported_alias_is_in_scope() {
    let seed = imported(&[("Handler", &[], "fn(string) -> nil"), ("Opt", &["T"], "T?")]);
    let module = parse("type Handlers = table<Handler>\ntype Name = Opt<string>\n");
    let (scope, errors) = alias_scope(&module, &seed);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(show(&scope["Handlers"].ty), "table<fn(string) -> nil>");
    assert_eq!(show(&scope["Name"].ty), "string?");
    assert!(scope.contains_key("Handler"), "the imported one stays in scope");
}

/// As with a class: what a module declares wins over what it imports.
#[test]
fn a_declared_alias_wins_over_an_imported_one() {
    let seed = imported(&[("Id", &[], "string")]);
    let module = parse("type Id = integer\ntype Ids = table<Id>\n");
    let (scope, errors) = alias_scope(&module, &seed);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(show(&scope["Id"].ty), "integer");
    assert_eq!(show(&scope["Ids"].ty), "table<integer>");
}

#[test]
fn a_declared_class_wins_over_an_imported_alias() {
    let seed = imported(&[("Event", &[], "table<string>")]);
    let module = parse("class Event\nend\nenum Kind\n  A\nend\ntype Events = table<Event>\n");
    let (scope, errors) = alias_scope(&module, &seed);
    assert!(errors.is_empty(), "{errors:?}");
    assert!(!scope.contains_key("Event"), "the class keeps its name");
    assert_eq!(show(&scope["Events"].ty), "table<Event>");
}

/// What an imported alias stands for is not read again: a name in it means
/// what it meant where the alias was declared.
#[test]
fn what_an_imported_alias_stands_for_is_not_expanded_again() {
    // Here `Event` is an alias; in the module `Handler` came from it was a
    // class of that name.
    let seed = imported(&[("Handler", &[], "fn(Event) -> nil")]);
    let src = "type Event = string\nlocal h: Handler = nil\nlocal e: Event = nil\n";
    let mut module = parse(src);
    let (scope, _) = alias_scope(&module, &seed);
    expand_aliases(&mut module, &scope);
    let annotation = |i: usize| match &module.stmts[i].value {
        Stmt::Local { ty: Some(t), .. } => show(t),
        other => panic!("expected a local, got {other:?}"),
    };
    assert_eq!(annotation(1), "fn(Event) -> nil");
    assert_eq!(annotation(2), "string");
}

#[test]
fn only_exported_aliases_are_exported() {
    let module = parse(
        "type Private = integer\nexport type Public = table<Private>\nexport type Other = string\n",
    );
    let (scope, _) = alias_scope(&module, &AliasRegistry::new());
    let exported = exported_aliases(&module, &scope);
    let mut names: Vec<&String> = exported.keys().collect();
    names.sort();
    assert_eq!(names, ["Other", "Public"]);
    // And in full: the private alias it was written with is gone.
    assert_eq!(show(&exported["Public"].ty), "table<integer>");
}

// ── mistakes in the declarations ──

#[test]
fn an_alias_of_itself_is_refused() {
    rejects("type Tree = table<Tree>\n", "defined in terms of itself");
    rejects("type Loop = Loop\n", "defined in terms of itself");
    rejects("type Maybe = Maybe?\n", "defined in terms of itself");
    rejects("type Call = fn(Call) -> nil\n", "defined in terms of itself");
}

#[test]
fn a_cycle_through_several_aliases_is_refused_once() {
    let errs = errors("type A = table<B>\ntype B = C?\ntype C = fn(A) -> nil\n");
    let cycles: Vec<String> = errs
        .iter()
        .map(|e| e.to_string())
        .filter(|m| m.contains("defined in terms of itself"))
        .collect();
    assert_eq!(cycles.len(), 1, "one cycle, one report: {cycles:?}");
    assert!(cycles[0].contains("`A`"), "{cycles:?}");
}

#[test]
fn a_cycle_through_a_generic_argument_is_refused() {
    rejects(
        "type Box<T> = table<T>\ntype Nest = Box<Nest>\n",
        "defined in terms of itself",
    );
}

/// The help spells out the path, so a long cycle can be followed.
#[test]
fn a_cycle_names_the_aliases_on_it() {
    use miette::Diagnostic;
    let errs = errors("type A = B\ntype B = C\ntype C = A\n");
    let help = errs
        .iter()
        .filter_map(|e| e.help().map(|h| h.to_string()))
        .find(|h| h.contains('→'))
        .expect("a cycle with its path");
    assert!(help.contains("`A` → `B` → `C` → `A`"), "{help}");
}

/// A cycle is settled as `any`, so the aliases beside it and the code
/// using it are checked as usual and say nothing more about it.
#[test]
fn a_cycle_does_not_spoil_the_aliases_beside_it() {
    let src = "type A = table<A>\ntype Id = integer\ntype Ids = table<Id>\nlocal x: A = nil\n";
    let (scope, errs) = scope_of(src);
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert_eq!(show(&scope["Ids"].ty), "table<integer>");
    assert_eq!(show(&scope["A"].ty), "any");
}

#[test]
fn an_alias_through_its_own_parameter_is_not_a_cycle() {
    // `T` here is the parameter, not a reference to anything.
    accepts("type T<T> = table<T>\n");
    accepts("type Wrap<Wrap> = table<Wrap>\n");
}

#[test]
fn an_alias_declared_twice_is_refused() {
    rejects("type Id = integer\ntype Id = string\n", "declared more than once");
}

#[test]
fn an_alias_cannot_share_a_name_with_another_type_of_the_module() {
    rejects("class Shape\nend\ntype Shape = integer\n", "declared more than once");
    rejects(
        "type Shape = integer\ninterface Shape\n  fn area() -> float\nend\n",
        "declared more than once",
    );
    rejects("enum Kind\n  A\nend\ntype Kind = integer\n", "declared more than once");
}

/// The first declaration stands, so everything written with the name is
/// still checked against something.
#[test]
fn the_first_of_two_declarations_stands() {
    let (scope, errs) = scope_of("type Id = integer\ntype Id = string\n");
    assert_eq!(errs.len(), 1);
    assert_eq!(show(&scope["Id"].ty), "integer");
}

#[test]
fn an_alias_cannot_take_the_name_of_a_built_in_type() {
    for name in [
        "integer", "float", "byte", "string", "bytes", "boolean", "table", "userdata", "thread",
        "any", "number", "function",
    ] {
        rejects(&format!("type {name} = string\n"), "already a type");
    }
}

// ── mistakes in the uses ──

#[test]
fn a_generic_alias_needs_all_its_arguments() {
    let src = "type Pair<A, B> = table<A, B>\n";
    rejects(&format!("{src}local p: Pair<string> = nil\n"), "takes 2 type arguments, but is used here with 1");
    rejects(&format!("{src}local p: Pair = nil\n"), "takes 2 type arguments, but is used here with none");
    rejects(
        &format!("{src}local p: Pair<string, integer, float> = nil\n"),
        "takes 2 type arguments, but is used here with 3",
    );
    accepts(&format!("{src}local p: Pair<string, integer> = nil\n"));
}

#[test]
fn a_plain_alias_takes_no_arguments() {
    rejects(
        "type Id = integer\nlocal i: Id<string> = nil\n",
        "takes no type arguments, but is used here with 1",
    );
}

#[test]
fn a_wrong_argument_count_is_reported_wherever_the_alias_is_written() {
    let src = "type Pair<A, B> = table<A, B>\n";
    rejects(&format!("{src}fn f(p: Pair<string>)\nend\n"), "type argument");
    rejects(&format!("{src}fn f() -> Pair<string>\n  return nil\nend\n"), "type argument");
    rejects(&format!("{src}class C\n  p: Pair<string> = nil\nend\n"), "type argument");
    rejects(&format!("{src}local x = nil as Pair<string>\n"), "type argument");
    rejects(&format!("{src}type Bad = table<Pair<string>>\n"), "type argument");
    rejects(
        &format!("{src}interface I\n  fn get() -> Pair<string>\nend\n"),
        "type argument",
    );
}

/// Once, where it is declared — not again for the alias built on it.
#[test]
fn a_wrong_argument_count_inside_an_alias_is_reported_once() {
    let errs = errors(
        "type Pair<A, B> = table<A, B>\ntype Bad = Pair<string>\nlocal a: Bad = nil\nlocal b: Bad = nil\n",
    );
    assert_eq!(errs.len(), 1, "{errs:?}");
}

// ── an alias is not a value ──

#[test]
fn an_alias_named_as_a_value_is_refused() {
    let src = "type Handler = fn() -> nil\n";
    rejects(&format!("{src}local h = Handler\n"), "is a type alias, not a value");
    rejects(&format!("{src}local h = Handler()\n"), "is a type alias, not a value");
    rejects(
        &format!("{src}fn f() -> nil\n  local t = {{ Handler }}\n  return\nend\n"),
        "is a type alias, not a value",
    );
}

/// The two kinds of name do not collide: a local or a parameter may share
/// an alias's name, and in an expression the name means the value.
#[test]
fn a_value_may_share_an_aliass_name() {
    accepts("type Size = integer\nfn area(Size: integer) -> integer\n  return Size * Size\nend\n");
    accepts(
        "type Size = integer\nfn f() -> integer\n  local Size: Size = 3\n  return Size\nend\n",
    );
    accepts("type Size = integer\nlocal Size: Size = 3\nlocal twice = Size + Size\n");
    accepts("type Size = integer\nfn Size() -> integer\n  return 1\nend\nlocal s = Size()\n");
}

#[test]
fn an_imported_alias_named_as_a_value_is_refused() {
    let module = parse("from events import Handler\nlocal h = Handler\n");
    let seed = ModuleSeed {
        aliases: imported(&[("Handler", &[], "fn() -> nil")]),
        ..ModuleSeed::default()
    };
    let errs = analyze_with_seed(&module, seed);
    assert!(
        errs.iter()
            .any(|e| e.to_string().contains("is a type alias, not a value")),
        "{errs:?}"
    );
}

#[test]
fn an_alias_inside_a_block_is_refused() {
    rejects(
        "fn f() -> nil\n  type Inner = integer\n  return\nend\n",
        "top level of a module",
    );
    rejects("if true then\n  type Inner = integer\nend\n", "top level of a module");
    rejects(
        "class C\n  fn m() -> nil\n    type Inner = integer\n    return\n  end\nend\n",
        "top level of a module",
    );
}

/// The tree a caller holds is not rewritten by analysis: a formatter prints
/// it and an editor shows it, and both want the names as they were typed.
#[test]
fn analysis_leaves_the_callers_tree_as_written() {
    let module = parse("type Id = integer\nlocal a: Id = 1\n");
    let _ = analyze(&module);
    match &module.stmts[1].value {
        Stmt::Local { ty: Some(t), .. } => assert_eq!(show(t), "Id"),
        other => panic!("expected a local, got {other:?}"),
    }
    // What it stands for is there to be asked for.
    let alias = saule_semantic::lookup_alias("Id").expect("installed by analysis");
    assert_eq!(show(&alias.ty), "integer");
}

/// And the registries the checker reads are built from the rewritten one.
#[test]
fn the_registries_hold_no_alias() {
    let module = parse(
        "type Id = integer\ntype Handler = fn(Id) -> nil\n\
         class Bus\n  last: Id = 0\n  fn on(h: Handler) -> Id\n    return 1\n  end\nend\n\
         fn make(seed: Id) -> Handler\n  return fn(i: integer) -> nil end\nend\n",
    );
    let _ = analyze(&module);
    let field = saule_semantic::lookup_field_type("Bus", "last").expect("a field");
    assert_eq!(show(&field), "integer");
    let method = saule_semantic::lookup_method("Bus", "on").expect("a method");
    assert_eq!(show(&method.params[0].ty), "fn(integer) -> nil");
    assert_eq!(method.return_ty.as_ref().map(show).as_deref(), Some("integer"));
    let function = saule_semantic::lookup_function("make").expect("a function");
    assert_eq!(show(&function.params[0].ty), "integer");
    assert_eq!(
        function.return_ty.as_ref().map(show).as_deref(),
        Some("fn(integer) -> nil")
    );
}

/// The scope belongs to the module last analysed, and goes with it.
#[test]
fn a_module_without_aliases_clears_the_installed_scope() {
    let _ = analyze(&parse("type Id = integer\n"));
    assert!(saule_semantic::lookup_alias("Id").is_some());
    let _ = analyze(&parse("local a = 1\n"));
    assert!(saule_semantic::lookup_alias("Id").is_none());
}
