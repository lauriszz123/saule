//! Integration tests for `saule_typeck::check`.
//!
//! These drive the public entry point over real source, after running
//! `saule_semantic::analyze` to populate the class / interface / enum
//! registries the checker reads — the same order the CLI and the LSP use.
//!
//! The emphasis is on the properties that make a static type system worth
//! having: that an annotation is a *guarantee*. Each `rejects` case is a
//! program that would misbehave at runtime if the checker let it through,
//! and several of them are regression tests for holes that were once open.
//!
//! Note that prelude names (`print`, `assert`, …) are supplied by the
//! interpreter through `sigs::set_initializer`, which isn't installed
//! here, so these tests deliberately avoid stdlib calls and exercise the
//! type rules directly.

use saule_ast::Module;
use saule_typeck::{TypeCheckError, check};

fn parse(src: &str) -> Module {
    let tokens = saule_lexer::Lexer::new(src)
        .tokenize()
        .unwrap_or_else(|e| panic!("source should lex: {e:?}\n{src}"));
    saule_parser::parse(tokens).unwrap_or_else(|e| panic!("source should parse: {e:?}\n{src}"))
}

fn errors(src: &str) -> Vec<TypeCheckError> {
    let module = parse(src);
    // Populate the registries the checker consults. Its diagnostics are
    // irrelevant here — the semantic pass has its own test suite.
    let _ = saule_semantic::analyze(&module);
    check(&module)
}

/// Assert `src` typechecks cleanly.
fn accepts(src: &str) {
    let errs = errors(src);
    assert!(
        errs.is_empty(),
        "expected no type errors, got {errs:?}\n--- source ---\n{src}"
    );
}

/// Assert `src` is rejected with a message mentioning `needle`.
fn rejects(src: &str, needle: &str) {
    let errs = errors(src);
    assert!(
        !errs.is_empty(),
        "expected a type error mentioning {needle:?}, got none\n--- source ---\n{src}"
    );
    let joined = errs
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(
        joined.contains(needle),
        "expected a type error mentioning {needle:?}, got: {joined}\n--- source ---\n{src}"
    );
}

/// Wrap `body` in a function so statements can be written directly.
fn in_fn(body: &str) -> String {
    format!("fn t() -> nothing\n{body}\n  return\nend\n")
}

// ─── primitive assignability ────────────────────────────────────────────

#[test]
fn matching_primitive_assignment_is_accepted() {
    accepts(&in_fn("  local n: integer = 1"));
    accepts(&in_fn("  local f: float = 1.5"));
    accepts(&in_fn("  local s: string = \"x\""));
    accepts(&in_fn("  local b: boolean = true"));
}

#[test]
fn mismatched_primitive_assignment_is_rejected() {
    rejects(&in_fn("  local n: integer = \"x\""), "integer");
}

#[test]
fn integer_and_float_do_not_silently_convert() {
    // The README is explicit that Saule never converts numerics silently.
    rejects(&in_fn("  local n: integer = 1.5"), "integer");
}

#[test]
fn arithmetic_on_a_string_is_rejected() {
    rejects(
        &in_fn("  local s: string = \"a\"\n  local n: integer = s + 1"),
        "operator `+` cannot be applied",
    );
}

// ─── nullability ────────────────────────────────────────────────────────

#[test]
fn nullable_cannot_fill_a_non_nullable_slot() {
    rejects(
        &in_fn("  local a: integer? = nil\n  local b: integer = a"),
        "nullable",
    );
}

#[test]
fn nil_cannot_fill_a_non_nullable_slot() {
    rejects(&in_fn("  local n: integer = nil"), "nil");
}

#[test]
fn coalesce_collapses_nullability() {
    accepts(&in_fn(
        "  local a: integer? = nil\n  local b: integer = a ?? 0",
    ));
}

#[test]
fn force_unwrap_collapses_nullability() {
    accepts(&in_fn("  local a: integer? = nil\n  local b: integer = a!"));
}

#[test]
fn narrowing_through_a_nil_check_is_accepted() {
    accepts(&in_fn(
        "  local a: integer? = nil\n  if a != nil then\n    local b: integer = a\n  end",
    ));
}

// ─── class subtyping and variance ───────────────────────────────────────

const HIERARCHY: &str = "\
class Animal
end
class Dog extends Animal
end
";

#[test]
fn a_subclass_is_assignable_to_its_parent_slot() {
    accepts(&format!(
        "{HIERARCHY}{}",
        in_fn("  local a: Animal = Dog()")
    ));
}

#[test]
fn a_parent_is_not_assignable_to_a_subclass_slot() {
    rejects(
        &format!("{HIERARCHY}{}", in_fn("  local d: Dog = Animal()")),
        "Dog",
    );
}

#[test]
fn function_return_types_are_covariant_not_contravariant() {
    // A `fn() -> Animal` cannot stand in for a `fn() -> Dog`: callers of
    // the latter would receive a bare Animal.
    rejects(
        &format!(
            "{HIERARCHY}{}",
            in_fn(
                "  local mk: fn() -> Animal = fn() -> Animal\n    return Animal()\n  end\n  \
                 local d: fn() -> Dog = mk"
            )
        ),
        "Dog",
    );
}

#[test]
fn overriding_with_a_narrower_parameter_is_rejected() {
    // Parameters are contravariant: `DogHandler.handle(Dog)` would break
    // any caller holding a `Handler` and passing a plain `Animal`.
    rejects(
        &format!(
            "{HIERARCHY}\
class Handler
  fn handle(a: Animal) -> nothing
    return
  end
end
class DogHandler extends Handler
  fn handle(d: Dog) -> nothing
    return
  end
end
"
        ),
        "handle",
    );
}

#[test]
fn overriding_with_an_identical_signature_is_accepted() {
    accepts(&format!(
        "{HIERARCHY}\
class Handler
  fn handle(a: Animal) -> nothing
    return
  end
end
class Sub extends Handler
  fn handle(a: Animal) -> nothing
    return
  end
end
"
    ));
}

#[test]
fn a_static_field_read_off_the_class_has_the_declared_type() {
    // Regression: the receiver of `Cursors.requested` is a class *name*, not a
    // value, so inferring it as an expression answered "no information" and
    // every annotated use failed with `UndeterminedType` — even from inside
    // the class that declared the field.
    accepts(
        "class Cursors\n\
        \x20 static local requested: string = \"\"\n\
        \x20 static fn apply() -> string\n\
        \x20   local wanted: string = Cursors.requested\n\
        \x20   return wanted\n\
        \x20 end\n\
        end\n",
    );
}

#[test]
fn a_static_field_read_still_has_to_match_the_slot() {
    // The inference above must not be a free pass: now that the read has a
    // type, a mismatched binding is a real error rather than a silent skip.
    rejects(
        "class Cursors\n\
        \x20 static requested: string = \"\"\n\
        end\n\
        fn t() -> integer\n\
        \x20 local n: integer = Cursors.requested\n\
        \x20 return n\n\
        end\n",
        "integer",
    );
}

#[test]
fn a_local_shadowing_a_class_name_wins_over_the_static_lookup() {
    // `Cursors` here is a table-typed local, so `.requested` is map sugar for
    // `Cursors["requested"]` and yields the table's value type — the class of
    // the same name must not hijack the read.
    rejects(
        "class Cursors\n\
        \x20 static requested: string = \"\"\n\
        end\n\
        fn t() -> integer\n\
        \x20 local Cursors: table<string, integer> = {}\n\
        \x20 local s: string = Cursors.requested\n\
        \x20 return 0\n\
        end\n",
        "string",
    );
}

// ─── tables ─────────────────────────────────────────────────────────────

#[test]
fn table_element_types_are_enforced() {
    rejects(&in_fn("  local xs: table<integer> = {1, \"two\"}"), "table");
}

#[test]
fn an_empty_literal_fills_any_table_slot() {
    // `{}` has no element type yet, so it must satisfy every table slot.
    accepts(&in_fn("  local xs: table<integer> = {}"));
    accepts(&in_fn("  local ys: table<string> = {}"));
}

#[test]
fn iterating_an_empty_literal_accepts_any_binding_type() {
    // Zero iterations means nothing can be bound, so nothing can be bound
    // wrongly — this must not be read as an `any` downcast.
    accepts(&in_fn("  for v: integer in {} do\n  end"));
}

#[test]
fn a_typed_table_cannot_be_aliased_as_table_any() {
    // Regression: tables are mutable and shared by reference, so widening
    // `table<integer>` to `table<any>` hands out a window through which
    // the container can be poisoned with values its element type forbids.
    rejects(
        &in_fn("  local nums: table<integer> = {1}\n  local anys: table<any> = nums"),
        "table",
    );
}

#[test]
fn a_table_literal_argument_is_checked_element_wise() {
    // Regression: the literal was inferred bottom-up as `table<Dog>` and
    // then compared against `table<Animal>` under the invariance rule, so
    // an argument position rejected the very literal a binding accepted.
    // A literal at the point of use has no second name to be reached by,
    // so there is no aliasing for invariance to protect against.
    accepts(&format!(
        "{HIERARCHY}\
class Pack
  fn init(members: table<Animal>)
    return
  end
end
{}",
        in_fn("  local p: Pack = Pack({ Dog() })")
    ));
}

#[test]
fn a_table_literal_argument_still_rejects_a_foreign_element() {
    // Element-wise checking is not a free pass — it points at the entry
    // that is actually wrong instead of at the whole braces.
    rejects(
        &format!(
            "{HIERARCHY}\
class Rock
end
class Pack
  fn init(members: table<Animal>)
    return
  end
end
{}",
            in_fn("  local p: Pack = Pack({ Dog(), Rock() })")
        ),
        "Animal",
    );
}

#[test]
fn a_named_table_argument_is_still_invariant() {
    // The soundness rule the literal case sidesteps must stay in force for
    // anything with a name: `Pack` could store a `Cat` through the wider
    // alias, which the `table<Dog>` binding forbids.
    rejects(
        &format!(
            "{HIERARCHY}\
class Pack
  fn init(members: table<Animal>)
    return
  end
end
{}",
            in_fn("  local dogs: table<Dog> = { Dog() }\n  local p: Pack = Pack(dogs)")
        ),
        "table",
    );
}

#[test]
fn a_member_read_off_a_bare_table_is_any() {
    // `data: table` has no element types, so `data.handle` is Lua map
    // sugar whose value is genuinely dynamic. That is `any` — the
    // checker's explicit "could be anything", which then has to go
    // through `as` — not an absence of type information.
    accepts(&format!(
        "{HIERARCHY}\
class El
  data: table
  fn init()
    self.data = {{}}
  end
end
{}",
        in_fn("  local e: El = El()\n  local a: Animal = (e.data.pet as Animal)!")
    ));
}

#[test]
fn a_bare_table_member_still_needs_a_cast() {
    // The `any` answer must not become a silent downcast: assigning it
    // straight into a concrete slot is the direction `as` exists for.
    rejects(
        &format!(
            "{HIERARCHY}\
class El
  data: table
  fn init()
    self.data = {{}}
  end
end
{}",
            in_fn("  local e: El = El()\n  local a: Animal = e.data.pet")
        ),
        "any",
    );
}

// ─── `any` soundness and the `as` escape ────────────────────────────────

#[test]
fn widening_into_any_is_always_allowed() {
    accepts(&in_fn("  local a: any = 5"));
    accepts(&in_fn("  local a: any = \"text\""));
    accepts(&in_fn("  local a: any = {1, 2}"));
}

#[test]
fn any_cannot_flow_into_a_concrete_slot() {
    // Regression: this used to be accepted, leaving a `string` sitting in
    // a variable annotated `integer` with no error anywhere.
    rejects(
        &in_fn("  local a: any = \"text\"\n  local n: integer = a"),
        "integer",
    );
}

#[test]
fn any_cannot_flow_into_a_nullable_concrete_slot_either() {
    rejects(
        &in_fn("  local a: any = \"text\"\n  local n: integer? = a"),
        "integer",
    );
}

#[test]
fn a_cast_yields_a_nullable_of_the_target_type() {
    accepts(&in_fn(
        "  local a: any = 5\n  local n: integer? = a as integer",
    ));
}

#[test]
fn a_cast_result_is_nullable_and_must_be_handled() {
    // `a as integer` is `integer?`, so it cannot fill a non-nullable slot
    // without `??` or `!`.
    rejects(
        &in_fn("  local a: any = 5\n  local n: integer = a as integer"),
        "nullable",
    );
}

#[test]
fn a_cast_composes_with_coalesce_and_force_unwrap() {
    accepts(&in_fn(
        "  local a: any = 5\n  local n: integer = a as integer ?? 0",
    ));
    accepts(&in_fn(
        "  local a: any = 5\n  local n: integer = (a as integer)!",
    ));
}

#[test]
fn casting_an_already_typed_value_is_rejected() {
    rejects(
        &in_fn("  local n: integer = 5\n  local s = n as string"),
        "already",
    );
}

#[test]
fn a_cast_binds_tighter_than_binary_operators() {
    // `a as integer ?? 0` must parse as `(a as integer) ?? 0`; if `as`
    // bound looser this would not typecheck as an integer.
    accepts(&in_fn(
        "  local a: any = 5\n  local n: integer = a as integer ?? 0\n  local m: integer = n + 1",
    ));
}

#[test]
fn casting_to_a_class_type_is_allowed() {
    accepts(&format!(
        "{HIERARCHY}{}",
        in_fn("  local a: any = Dog()\n  local d: Dog? = a as Dog")
    ));
}

// ─── calls ──────────────────────────────────────────────────────────────

#[test]
fn argument_types_are_checked() {
    rejects(
        "fn f(n: integer) -> nothing\n  return\nend\nfn t() -> nothing\n  f(\"x\")\n  return\nend\n",
        "integer",
    );
}

#[test]
fn too_many_arguments_are_rejected() {
    rejects(
        "fn f(n: integer) -> nothing\n  return\nend\nfn t() -> nothing\n  f(1, 2)\n  return\nend\n",
        "argument",
    );
}

#[test]
fn a_returned_value_must_match_the_declared_return_type() {
    rejects("fn f() -> integer\n  return \"x\"\nend\n", "integer");
}

#[test]
fn a_nil_match_arm_does_not_widen_the_result_to_any() {
    // Regression: `case _ then nil` used to contribute `nil` as an arm *base*,
    // which then disagreed with the `string` arms and widened the whole match
    // to `any?` — rejecting a correct `-> string?` function.
    accepts(
        "fn f(e: integer) -> string?\n  return match e\n\
         \x20   case 110 then \"\\n\"\n\
         \x20   case 116 then \"\\t\"\n\
         \x20   case _ then nil\n  end\nend\n",
    );
}

#[test]
fn genuinely_mixed_match_arms_still_widen() {
    // The nil-arm fix must not disable the widening it was hiding: arms of
    // truly different shapes are still `any`, so a `string?` return is wrong.
    rejects(
        "fn f(e: integer) -> string?\n  return match e\n\
         \x20   case 110 then \"\\n\"\n\
         \x20   case 116 then 1\n\
         \x20   case _ then nil\n  end\nend\n",
        "string?",
    );
}

#[test]
fn default_parameters_may_be_omitted() {
    accepts(
        "fn f(a: integer, b: integer = 2) -> integer\n  return a + b\nend\n\
         fn t() -> integer\n  return f(1)\nend\n",
    );
}

// ─── generics ───────────────────────────────────────────────────────────

#[test]
fn a_generic_identity_function_preserves_its_argument_type() {
    accepts(
        "fn id<T>(v: T) -> T\n  return v\nend\n\
         fn t() -> nothing\n  local n: integer = id(1)\n  return\nend\n",
    );
}

#[test]
fn a_generic_return_is_checked_against_the_binding() {
    rejects(
        "fn id<T>(v: T) -> T\n  return v\nend\n\
         fn t() -> nothing\n  local s: string = id(1)\n  return\nend\n",
        "string",
    );
}

/// A type parameter has to be able to bind to a *nullable* type. This is
/// the shape of `Table.insert<V>(table<V>, V)` called with a `table<any?>`:
/// binding `V := any` instead of `V := any?` left `V` unbound (the binder
/// refuses to bind `any`) and every argument was then rejected against the
/// bare parameter name.
#[test]
fn a_type_parameter_binds_to_a_nullable_element_type() {
    accepts(
        "fn push<V>(t: table<V>, v: V) -> nothing\n  return\nend\n\
         fn t() -> nothing\n  local a: table<any?> = {}\n  local v: any? = nil\n  \
         push(a, v)\n  return\nend\n",
    );
}

#[test]
fn a_nullable_element_type_still_constrains_later_arguments() {
    rejects(
        "fn push<V>(t: table<V>, v: V) -> nothing\n  return\nend\n\
         fn t() -> nothing\n  local a: table<string?> = {}\n  push(a, 42)\n  return\nend\n",
        "string?",
    );
}

/// A type parameter the arguments haven't pinned down yet is *unknown*, not
/// a concrete type that happens to be spelled `V`. `table<any>` never binds
/// it (binding `V := any` would erase the constraint for later arguments),
/// so the element slot has to stay permissive rather than reject everything.
#[test]
fn an_unbound_type_parameter_accepts_an_any_argument() {
    accepts(
        "fn push<V>(t: table<V>, v: V) -> nothing\n  return\nend\n\
         fn t() -> nothing\n  local a: table<any> = {}\n  local x: any = 1\n  \
         push(a, x)\n  return\nend\n",
    );
}

/// …but only at the parameter's own position. The structure around it is
/// still checked, so a non-table can't fill a `table<V>` slot.
#[test]
fn an_unbound_type_parameter_still_checks_the_surrounding_shape() {
    rejects(
        "fn push<V>(t: table<V>, v: V) -> nothing\n  return\nend\n\
         fn t() -> nothing\n  local n: integer = 1\n  push(n, 1)\n  return\nend\n",
        "table<V>",
    );
}

// ─── interfaces ─────────────────────────────────────────────────────────

#[test]
fn a_class_is_assignable_to_an_interface_it_implements() {
    accepts(
        "\
interface Drawable
  fn draw() -> nothing
end
class Square implements Drawable
  fn draw() -> nothing
    return
  end
end
fn t() -> nothing
  local d: Drawable = Square()
  return
end
",
    );
}

#[test]
fn a_class_is_not_assignable_to_an_unimplemented_interface() {
    rejects(
        "\
interface Drawable
  fn draw() -> nothing
end
class Blob
end
fn t() -> nothing
  local d: Drawable = Blob()
  return
end
",
        "Drawable",
    );
}

// ─── returns ────────────────────────────────────────────────────────────

/// A class with an `any?` slot — the shape that made the hole visible: a
/// read off it is `any?`, which no concrete return type accepts.
const BOX: &str = "\
class Box
  slot: any?
  fn init()
    self.slot = nil
  end
end
";

#[test]
fn a_return_of_a_local_declared_in_a_nested_block_is_checked() {
    // Regression: returns used to be checked by a *second* walk over the
    // body, carrying the scope the first walk finished with. That scope
    // held the parameters and the body's top-level locals, but nothing
    // bound inside an `if` or a `while` — those went into clones that were
    // dropped. So `infer` answered `None` for such a name and the check
    // was skipped, letting an `any` reach a concrete return type and fail
    // at runtime instead.
    for block in ["if true then", "while true do", "for i = 1, 3 do"] {
        rejects(
            &format!(
                "{BOX}\
fn probe(b: Box) -> string
  {block}
    local found = b.slot
    return found
  end
  return \"x\"
end
"
            ),
            "string",
        );
    }
}

#[test]
fn a_return_of_a_top_level_local_is_still_checked() {
    // The case that always worked must keep working.
    rejects(
        &format!(
            "{BOX}\
fn probe(b: Box) -> string
  local found = b.slot
  return found
end
"
        ),
        "string",
    );
}

#[test]
fn a_lambda_return_is_not_checked_against_the_enclosing_function() {
    // A `return` inside a lambda returns from the lambda. Checking it
    // against the enclosing signature would reject correct code — the
    // failure mode of hanging the return type on a thread-local without
    // pushing a fresh one per body.
    accepts(
        "fn outer() -> string\n\
        \x20 local pick: fn() -> integer = fn()\n\
        \x20   return 42\n\
        \x20 end\n\
        \x20 return \"ok\"\n\
        end\n",
    );
}

#[test]
fn a_lambda_with_a_declared_return_type_is_still_checked() {
    // Isolation must not become exemption — including for a `return`
    // nested inside a block within the lambda.
    rejects(
        "fn outer() -> string\n\
        \x20 local bad: fn() -> integer = fn()\n\
        \x20   if true then\n\
        \x20     local s = \"nope\"\n\
        \x20     return s\n\
        \x20   end\n\
        \x20   return 0\n\
        \x20 end\n\
        \x20 return \"ok\"\n\
        end\n",
        "integer",
    );
}

#[test]
fn a_method_return_is_checked_inside_nested_blocks_too() {
    // The method walk sets and restores the return type the same way the
    // free-function walk does; `Theme.of`'s shape is a static method with
    // the return buried in a loop.
    rejects(
        &format!(
            "{BOX}\
class T
  static fn of(b: Box) -> string
    while true do
      local found = b.slot
      if found != nil then
        return found
      end
    end
    return \"x\"
  end
end
"
        ),
        "string",
    );
}
