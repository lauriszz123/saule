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
fn casting_to_the_type_a_value_already_has_is_rejected() {
    rejects(
        &in_fn("  local n: integer = 5\n  local m = n as integer"),
        "already",
    );
}

/// The other end of the rule: a pair with no conversion is an error too,
/// rather than a cast that quietly produces `nil` forever.
#[test]
fn a_cast_with_no_conversion_is_rejected() {
    rejects(
        &in_fn("  local n: integer = 5\n  local b = n as boolean"),
        "no cast",
    );
}

/// A typed value converts. `int()` / `float()` used to be the only way to
/// say these, and the cast now says them without a call.
#[test]
fn a_typed_value_converts() {
    accepts(&in_fn("  local x: integer = 10f as integer"));
    accepts(&in_fn("  local y: float = 3 as float"));
    accepts(&in_fn("  local s: string = 42 as string"));
    // Parsing is the direction that can fail, so it stays nullable.
    accepts(&in_fn("  local n: integer? = \"42\" as integer"));
    rejects(&in_fn("  local n: integer = \"42\" as integer"), "nullable");
    // As does a conversion off a nullable operand.
    accepts(&in_fn(
        "  local f: float? = 1.5\n  local n: integer? = f as integer",
    ));
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

// ── calls through function-typed values, and rigid type parameters ─────

#[test]
fn applying_a_callback_to_the_wrong_type_param_is_rejected() {
    // The motivating case: `f` is declared `fn(U) -> U` but applied to an
    // element of `table<T>`. Nothing proves a `T` is a `U`, and the call
    // goes through a *value* of function type, which used to be checked
    // nowhere at all.
    rejects(
        "fn map<T, U>(items: table<T>, f: fn(U) -> U) -> table<U>\n\
        \x20 local out: table<U> = {}\n\
        \x20 for item: T in items do\n\
        \x20   out[#out + 1] = f(item)\n\
        \x20 end\n\
        \x20 return out\n\
        end\n",
        "argument 1 of `f`",
    );
}

#[test]
fn the_same_function_with_matching_type_params_is_accepted() {
    // The corrected signature — `f: fn(T) -> U` — must stay clean, or the
    // rule above would make generic higher-order code unwritable.
    accepts(
        "fn map<T, U>(items: table<T>, f: fn(T) -> U) -> table<U>\n\
        \x20 local out: table<U> = {}\n\
        \x20 for item: T in items do\n\
        \x20   out[#out + 1] = f(item)\n\
        \x20 end\n\
        \x20 return out\n\
        end\n",
    );
}

#[test]
fn one_type_param_does_not_satisfy_another() {
    // The same rule outside a call: two type parameters are independent,
    // so neither can fill the other's slot.
    rejects(
        "fn convert<T, U>(a: T) -> U\n\
        \x20 local x: U = a\n\
        \x20 return x\n\
        end\n",
        "`U`",
    );
}

#[test]
fn a_type_param_slot_accepts_a_value_of_that_same_param() {
    // What a rigid `T` *does* accept: another `T`. This is the shape
    // nearly all generic code is built from, and it must stay clean.
    accepts(
        "fn wrap<T>(x: T) -> table<T>\n\
        \x20 local out: table<T> = {}\n\
        \x20 out[1] = x\n\
        \x20 return out\n\
        end\n",
    );
}

/// A type parameter is universally quantified: the *caller* decides what
/// it stands for, so inside the body nothing proves a `T` is an
/// `integer`. Letting it through made the annotation a lie — the value
/// flowed on unchecked and failed much later, at the first operation
/// that cared about the real type.
#[test]
fn a_type_param_does_not_satisfy_a_concrete_type() {
    rejects(
        "fn firstAsInt<T>(items: table<T>) -> integer\n\
        \x20 for item: T in items do\n\
        \x20   local n: integer = item\n\
        \x20   return n\n\
        \x20 end\n\
        \x20 return 0\n\
        end\n",
        "`integer`",
    );
}

/// And the reverse: `T` may well be `string`, so an `integer` cannot
/// fill a `T` slot either.
#[test]
fn a_concrete_type_does_not_satisfy_a_type_param() {
    rejects(
        "fn seed<T>() -> T\n\
        \x20 local acc: T = 0\n\
        \x20 return acc\n\
        end\n",
        "`T`",
    );
}

/// Widening stays free — `any` accepts everything, type parameters
/// included. This is what keeps `print(item)` and every other
/// `any`-taking call writable inside a generic body.
#[test]
fn a_type_param_widens_into_any() {
    accepts(
        "fn box<T>(x: T) -> any\n\
        \x20 local slot: any = x\n\
        \x20 return slot\n\
        end\n",
    );
}

/// With the direct assignment closed, `as` is how a body narrows a `T` —
/// the same checked escape `any` uses. It yields `integer?`, so the
/// failure case has to be handled.
#[test]
fn a_type_param_can_be_narrowed_with_a_checked_cast() {
    accepts(
        "fn firstAsInt<T>(items: table<T>) -> integer\n\
        \x20 for item: T in items do\n\
        \x20   local n: integer? = item as integer\n\
        \x20   return n ?? 0\n\
        \x20 end\n\
        \x20 return 0\n\
        end\n",
    );
}

/// Opening `as` up to type parameters must not open it to everything: on
/// a value whose type is already known the cast is a conversion, and it is
/// held to the conversion table rather than narrowing anything.
#[test]
fn a_cast_on_a_concrete_value_converts_instead_of_narrowing() {
    // `string` -> `integer` is a parse, and a parse can fail: `integer?`.
    accepts(
        "fn ok(x: string) -> integer\n\
        \x20 local n: integer? = x as integer\n\
        \x20 return n ?? 0\n\
        end\n",
    );
    rejects(
        "fn nope(x: string) -> integer\n\
        \x20 local n: integer = x as integer\n\
        \x20 return n\n\
        end\n",
        "nullable",
    );
    // And a pair off the table is still an error, type parameters or not.
    rejects(
        "fn nope(x: string) -> boolean\n\
        \x20 local b: boolean = x as boolean\n\
        \x20 return b\n\
        end\n",
        "no cast",
    );
}

/// A generic *signature* being called into is the opposite case: its
/// parameters are inference variables, so one the arguments haven't
/// pinned down still binds to whatever this position supplies. Sharing
/// one set with the body's rigid parameters is what made the rule above
/// unenforceable.
#[test]
fn an_unbound_param_of_a_called_signature_stays_permissive() {
    accepts(
        "fn insert<V>(into: table<V>, item: V) -> nothing\n\
        end\n\
        fn fill(bag: table<any>, x: any) -> nothing\n\
        \x20 insert(bag, x)\n\
        end\n",
    );
}

/// Everybody names their type parameter `T`, and two of them are still
/// unrelated. The callee's parameters are renamed apart before the call
/// is checked, so the caller's rigid `T` stays rigid — comparing by
/// spelling alone let a `T` fill an `integer` slot whenever the callee
/// happened to declare a `T` of its own.
#[test]
fn a_callee_sharing_a_param_name_does_not_soften_the_callers() {
    rejects(
        "fn takesInt<T>(n: integer, item: T) -> integer\n\
        \x20 return n\n\
        end\n\
        fn caller<T>(x: T) -> integer\n\
        \x20 return takesInt(x, x)\n\
        end\n",
        "expects `integer`",
    );
}

/// The renaming must not cost the legitimate case: a callee parameter
/// binds to the caller's `T` exactly as before, whatever either calls it.
#[test]
fn a_callee_param_still_binds_to_the_callers_type_param() {
    accepts(
        "fn wrap<T>(item: T) -> table<T>\n\
        \x20 local out: table<T> = {}\n\
        \x20 out[1] = item\n\
        \x20 return out\n\
        end\n\
        fn caller<T>(x: T) -> table<T>\n\
        \x20 return wrap(x)\n\
        end\n",
    );
}

/// An untyped lambda parameter takes its type from the slot it fills.
/// It parses as `any`, and the callee's signature is the only place the
/// real type can come from — so a misuse inside the body is caught
/// instead of being absorbed by `any`.
#[test]
fn a_lambda_argument_param_takes_the_declared_slot_type() {
    rejects(
        "fn takesInt(n: integer) -> integer\n\
        \x20 return n\n\
        end\n\
        fn apply(f: fn(string) -> integer) -> integer\n\
        \x20 return 0\n\
        end\n\
        fn run() -> integer\n\
        \x20 return apply(s => takesInt(s))\n\
        end\n",
        "expects `integer`",
    );
}

/// …and the correct body is accepted, with the parameter's real type
/// driving the check rather than everything being permitted.
#[test]
fn a_lambda_argument_body_checks_against_the_real_param_type() {
    accepts(
        "fn takesStr(s: string) -> integer\n\
        \x20 return 1\n\
        end\n\
        fn apply(f: fn(string) -> integer) -> integer\n\
        \x20 return 0\n\
        end\n\
        fn run() -> integer\n\
        \x20 return apply(s => takesStr(s))\n\
        end\n",
    );
}

/// A pipeline stage binds its generics from the value flowing in, so the
/// predicate's parameter is concrete by the time the body is checked.
#[test]
fn a_pipe_stage_lambda_param_is_bound_from_the_piped_value() {
    rejects(
        "fn takesStr(s: string) -> boolean\n\
        \x20 return true\n\
        end\n\
        fn filter<T>(items: table<T>, predicate: fn(T) -> boolean) -> table<T>\n\
        \x20 return items\n\
        end\n\
        fn run() -> table<integer>\n\
        \x20 return when({1, 2}):filter(x => takesStr(x))\n\
        end\n",
        "expects `string`",
    );
}

/// An unbound parameter offers no expectation, so the body is checked as
/// permissively as before rather than against a bare parameter name.
#[test]
fn an_unbound_slot_leaves_a_lambda_param_alone() {
    accepts(
        "fn build<T>(seed: integer, make: fn(T) -> T) -> integer\n\
        \x20 return seed\n\
        end\n\
        fn run() -> integer\n\
        \x20 return build(1, x => x)\n\
        end\n",
    );
}

/// The renamed parameter is an internal detail — a diagnostic quotes the
/// name the signature actually spells.
#[test]
fn a_diagnostic_never_shows_the_renamed_parameter() {
    let errs = errors(
        "fn takesInt<T>(n: integer, item: T) -> integer\n\
        \x20 return n\n\
        end\n\
        fn caller<T>(x: T) -> integer\n\
        \x20 return takesInt(x, x)\n\
        end\n",
    );
    let joined = errs
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(!joined.contains('$'), "renaming leaked into: {joined}");
}

#[test]
fn a_call_through_a_function_value_checks_argument_types() {
    rejects(
        "fn apply(f: fn(integer) -> integer) -> integer\n\
        \x20 return f(\"nope\")\n\
        end\n",
        "argument 1 of `f`",
    );
}

#[test]
fn a_call_through_a_function_value_checks_arity() {
    // A function type has no defaults and no variadic slot, so its arity
    // is exact.
    rejects(
        "fn apply(f: fn(integer) -> integer) -> integer\n\
        \x20 return f(1, 2)\n\
        end\n",
        "expects 1 argument",
    );
}

#[test]
fn a_call_through_a_function_local_is_checked_too() {
    // Not just parameters — any binding of function type.
    rejects(
        "fn main()\n\
        \x20 local g: fn(integer) -> integer = x => x\n\
        \x20 local r = g(\"s\")\n\
        end\n",
        "argument 1 of `g`",
    );
}

#[test]
fn a_function_value_call_yields_its_declared_return_type() {
    // The call's result type is now known, so the binding it feeds is
    // checked rather than skipped.
    rejects(
        "fn apply(f: fn(integer) -> string) -> integer\n\
        \x20 local n: integer = f(1)\n\
        \x20 return n\n\
        end\n",
        "string",
    );
}

#[test]
fn a_bare_function_annotation_is_rejected() {
    // `function` carried no parameter list, so there was nothing to check
    // against and `f(1, 2, 3)` went through unexamined. The spelling is gone:
    // a slot holding a callable has to name the signature it accepts.
    rejects(
        "fn apply(f: function) -> integer\n\
        \x20 f(1, 2, 3)\n\
        \x20 return 1\n\
        end\n",
        "`function` is not a type",
    );
}

#[test]
fn a_bare_function_annotation_is_rejected_in_every_position() {
    for src in [
        "local f: function = fn() -> nil end\n",
        "local g: function? = nil\n",
        "export h: function? = nil\n",
        "fn make() -> function\n\x20 return fn() -> nil end\n end\n",
        "fn take(f: (function)?) -> nil\n end\n",
        "fn nested(fs: table<function>) -> nil\n end\n",
        "fn inner(f: fn(function) -> nil) -> nil\n end\n",
        "class C\n\x20 cb: function?\n end\n",
        "interface I\n\x20 fn cb() -> function\n end\n",
        "enum E\n\x20 Handler(f: function)\n end\n",
        "fn cast(v: any) -> nil\n\x20 local f = v as function\n end\n",
        "fn caught() -> nil\n\x20 try\n\x20 catch e: function\n\x20 end\n end\n",
        "fn iter(xs: table<any>) -> nil\n\x20 for x: function in xs do\n\x20 end\n end\n",
        "fn lam() -> nil\n\x20 local f = fn() -> function\n\x20   return fn() -> nil end\n\x20 end\n end\n",
    ] {
        rejects(src, "`function` is not a type");
    }
}

#[test]
fn a_variant_payload_binds_at_its_declared_type() {
    // The registry records each tuple variant's field types, so a match arm
    // binds the payload as declared rather than as `any`. Using `w` and `h`
    // as floats has to need no conversion.
    accepts(
        "enum Shape\n\
        \x20 Rect(w: float, h: float),\n\
        \x20 Empty\n\
        end\n\
        fn area(s: Shape) -> float\n\
        \x20 return match s\n\
        \x20   case Shape.Rect(w, h) then w * h\n\
        \x20   case Shape.Empty then 0.0\n\
        \x20 end\n\
        end\n",
    );
}

#[test]
fn a_variant_payload_used_at_the_wrong_type_is_rejected() {
    // The other half of the guarantee: binding at the declared type is only
    // worth anything if a mismatch is caught. This passed silently while the
    // payload came through as `any`.
    rejects(
        "enum Shape\n\
        \x20 Rect(w: float, h: float)\n\
        end\n\
        fn label(text: string) -> string\n\
        \x20 return text\n\
        end\n\
        fn describe(s: Shape) -> string\n\
        \x20 return match s\n\
        \x20   case Shape.Rect(w, _) then label(w)\n\
        \x20 end\n\
        end\n",
        "argument 1 of `label`",
    );
}

#[test]
fn variant_payload_types_follow_the_declaration_order() {
    // A slot is typed by its position, so reading the first one as if it were
    // the second is an error even though both names are bound.
    rejects(
        "enum Tagged\n\
        \x20 Pair(name: string, count: integer)\n\
        end\n\
        fn double(n: integer) -> integer\n\
        \x20 return n + n\n\
        end\n\
        fn total(t: Tagged) -> integer\n\
        \x20 return match t\n\
        \x20   case Tagged.Pair(name, _) then double(name)\n\
        \x20 end\n\
        end\n",
        "argument 1 of `double`",
    );
}

#[test]
fn a_bare_variant_binds_nothing_and_stays_accepted() {
    // Variants with no payload have no fields to type; they must keep working.
    accepts(
        "enum Flag\n\
        \x20 On,\n\
        \x20 Off\n\
        end\n\
        fn pick(f: Flag) -> integer\n\
        \x20 return match f\n\
        \x20   case Flag.On then 1\n\
        \x20   case Flag.Off then 0\n\
        \x20 end\n\
        end\n",
    );
}

// ─── `when(...)` pipelines over generic stages ──────────────────────────────

/// The declarations the pipeline tests below chain together: a generic
/// `filter` that preserves its element type and a generic `map` that
/// changes it.
const GENERIC_STAGES: &str = "fn filter<T>(items: table<T>, p: fn(T) -> boolean) -> table<T>\n\
    \x20 return items\n\
    end\n\
    fn map<T, U>(items: table<T>, f: fn(T) -> U) -> table<U>\n\
    \x20 local out: table<U> = {}\n\
    \x20 return out\n\
    end\n";

#[test]
fn a_generic_stage_binds_its_type_params_from_the_piped_value() {
    // `filter` declares `table<T>`; the piped `table<integer>` is what says
    // what `T` is. Comparing the two as unrelated concrete types rejected
    // every generic stage that was ever written.
    accepts(&format!(
        "{GENERIC_STAGES}local doubled = when({{1, 2, 3}}):filter(x => x % 2 == 0)\n"
    ));
}

#[test]
fn a_pipeline_threads_the_instantiated_type_into_the_next_stage() {
    // `filter` hands on `table<integer>`, not its own `table<T>` — which is
    // exactly what `map`'s `table<T>` slot then binds against.
    accepts(&format!(
        "{GENERIC_STAGES}local doubled: table<integer> = when({{1, 2, 3}})\n\
        \x20 :filter(x => x % 2 == 0)\n\
        \x20 :map(x => x * 2)\n"
    ));
}

#[test]
fn a_generic_stage_still_rejects_a_structurally_wrong_value() {
    // Binding `T` from the piped value is not the same as accepting
    // anything: a `string` has no element type for `table<T>` to bind.
    rejects(
        &format!("{GENERIC_STAGES}local err = when(\"hello\"):filter(x => true)\n"),
        "pipeline stage `filter`",
    );
}

#[test]
fn the_instantiated_type_is_what_a_later_stage_is_checked_against() {
    // `map` over a `table<integer>` produces `table<integer>`, and `join`
    // wants strings. The mismatch is only visible once the chain carries
    // instantiated types rather than parameter names.
    rejects(
        &format!(
            "{GENERIC_STAGES}fn join(parts: table<string>) -> string\n\
            \x20 return \"\"\n\
            end\n\
            local err = when({{1, 2}}):map(n => n * 2):join()\n"
        ),
        "pipeline stage `join`",
    );
}

#[test]
fn an_unbound_stage_type_param_does_not_leak_into_the_next_check() {
    // Nothing pins `U` down here, so the chain's type is unknown rather
    // than a literal `table<U>` — and an unknown value must not manufacture
    // a mismatch against the following stage.
    accepts(&format!(
        "{GENERIC_STAGES}fn make<V>(items: table<integer>) -> table<V>\n\
        \x20 local out: table<V> = {{}}\n\
        \x20 return out\n\
        end\n\
        fn join(parts: table<string>) -> string\n\
        \x20 return \"\"\n\
        end\n\
        local err = when({{1, 2}}):make():join()\n"
    ));
}

// ─── a lambda's declared return type binds its body ──────────────────────

/// The point of removing the bare `function` type: a lambda stored in a
/// declared callback slot is checked against that slot's signature. Under
/// `function` all four of these were accepted, and the mismatch surfaced at
/// the call — or, for a callback invoked from somewhere else entirely, never.
#[test]
fn rejects_a_lambda_that_does_not_match_the_declared_callback() {
    let field = "\
class Field
  onChanged: (fn(string) -> nil)?

  fn init()
    self.onChanged = nil
  end
end
";
    // Wrong parameter type.
    rejects(
        &format!(
            "{field}fn t() -> nothing\n  local f: Field = Field()\n  f.onChanged = fn(n: integer)\n    print(n)\n  end\n  return\nend\n"
        ),
        "(fn(string) -> nil)?",
    );
    // Wrong arity.
    rejects(
        &format!(
            "{field}fn t() -> nothing\n  local f: Field = Field()\n  f.onChanged = fn(a: string, b: string)\n    print(a)\n  end\n  return\nend\n"
        ),
        "(fn(string) -> nil)?",
    );
    // Wrong return type.
    rejects(
        &format!(
            "{field}fn t() -> nothing\n  local f: Field = Field()\n  f.onChanged = fn(s: string) -> integer\n    return 1\n  end\n  return\nend\n"
        ),
        "(fn(string) -> nil)?",
    );
    // And the matching one still passes, with the parameter type supplied by
    // the slot rather than written out.
    accepts(&format!(
        "{field}fn t() -> nothing\n  local f: Field = Field()\n  f.onChanged = s => print(s)\n  return\nend\n"
    ));
}

/// A lambda that declares `-> T` must actually return a `T`. The declaration
/// used to be checked against the target's signature but never against the
/// body, so the annotation guaranteed nothing about what came back.
#[test]
fn rejects_lambda_body_violating_its_own_return_type() {
    rejects(
        "\
fn apply(f: fn(integer) -> integer) -> integer
  return f(1)
end

fn t() -> nothing
  apply(fn(n) -> integer
    return \"nope\"
  end)
  return
end
",
        "return",
    );
}

/// Same hole through the trailing-block spelling — it is the same tree, so it
/// must produce the same error.
#[test]
fn rejects_trailing_block_body_violating_its_own_return_type() {
    rejects(
        "\
fn apply(f: fn(integer) -> integer) -> integer
  return f(1)
end

fn t() -> nothing
  apply() do (n) -> integer
    return \"nope\"
  end
  return
end
",
        "return",
    );
}

/// With no target type to check against, the lambda's own annotation is still
/// the contract its body has to meet.
#[test]
fn rejects_untargeted_lambda_body_violating_its_own_return_type() {
    rejects(
        &in_fn("  local f = fn(n: integer) -> integer\n    return \"nope\"\n  end"),
        "return",
    );
}

/// A lambda that omits its return type still inherits the target's, so the
/// pre-existing inference path is untouched.
#[test]
fn rejects_lambda_body_violating_the_targets_return_type() {
    rejects(
        "\
fn apply(f: fn(integer) -> integer) -> integer
  return f(1)
end

fn t() -> nothing
  apply(fn(n)
    return \"nope\"
  end)
  return
end
",
        "return",
    );
}

/// The honest cases keep passing: a declared return type that the body meets,
/// through both spellings.
#[test]
fn accepts_lambda_and_trailing_block_meeting_their_return_type() {
    accepts(
        "\
fn apply(f: fn(integer) -> integer) -> integer
  return f(1)
end

fn t() -> nothing
  apply(fn(n) -> integer
    return n * 2
  end)
  apply() do (n) -> integer
    return n * 3
  end
  return
end
",
    );
}

/// A trailing block binds to the callee's last **function-typed** parameter,
/// not to the last parameter outright. `MenuItem`'s callback is followed by a
/// `boolean`, and binding the block there reported the block as a `boolean`
/// mismatch — for a call that is perfectly well-formed.
#[test]
fn accepts_a_trailing_block_when_a_non_callback_parameter_follows_it() {
    accepts(
        "\
class MenuItem
  label: string
  onSelected: fn() -> nil
  enabled: boolean

  fn init(label: string = \"\", onSelected: fn() -> nil = () => nil, enabled: boolean = true)
    self.label = label
    self.onSelected = onSelected
    self.enabled = enabled
  end
end

fn t() -> nothing
  local a: MenuItem = MenuItem(\"Open\") do
    print(\"open\")
  end
  return
end
",
    );
}

/// Same call written with the lambda inside the parentheses — the same tree,
/// so it must resolve to the same slot.
#[test]
fn accepts_a_paren_form_callback_when_a_non_callback_parameter_follows_it() {
    accepts(
        "\
class MenuItem
  onSelected: fn() -> nil
  enabled: boolean

  fn init(label: string = \"\", onSelected: fn() -> nil = () => nil, enabled: boolean = true)
    self.onSelected = onSelected
    self.enabled = enabled
  end
end

fn t() -> nothing
  local a: MenuItem = MenuItem(label: \"Open\", fn()
    print(\"open\")
  end)
  return
end
",
    );
}

/// The callback slot has to be *free*. Here a positional argument already
/// filled it, so the block falls through to the last parameter and the
/// mismatch is reported rather than silently absorbed.
#[test]
fn rejects_a_trailing_block_when_the_callback_slot_is_already_taken() {
    rejects(
        "\
class MenuItem
  fn init(label: string = \"\", onSelected: fn() -> nil = () => nil, enabled: boolean = true)
  end
end

fn t() -> nothing
  local a: MenuItem = MenuItem(\"Open\", () => nil) do
    print(\"open\")
  end
  return
end
",
        "boolean",
    );
}

/// An expression-bodied lambda was always checked against the return type its
/// target supplies; guard against a regression from the block-body fix.
#[test]
fn rejects_arrow_lambda_violating_its_return_type() {
    rejects(
        &in_fn("  local f: fn(integer) -> integer = n => \"nope\""),
        "integer",
    );
}

// ── Compound assignment ──────────────────────────────────────────────────
//
// `a op= b` is typed as `a = a op b`: the operator's own operand rules
// apply, and the *result* is what the target's declared type must accept.

#[test]
fn compound_assignment_accepts_matching_types() {
    accepts("local n: integer = 1\nn += 2");
    accepts("local f: float = 1.0\nf *= 2.0");
    accepts("local s: string = \"a\"\ns ..= \"b\"");
    accepts("local n: integer = 2\nn ^= 3");
}

#[test]
fn compound_assignment_enforces_operand_rules() {
    rejects("local s: string = \"a\"\ns += 1", "`+`");
    rejects("local n: integer = 1\nn *= \"x\"", "`*`");
}

#[test]
fn compound_assignment_does_not_promote_numeric_kinds() {
    // Same rule as `n = n / 2.0`: Saule never mixes `integer` and `float`.
    rejects("local n: integer = 1\nn /= 2.0", "integer");
    rejects("local f: float = 1.0\nf += 1", "float");
}

#[test]
fn compound_assignment_checks_the_result_against_the_target_type() {
    // `..` on an integer is legal, but the `string` result is not
    // assignable back into an `integer` binding.
    rejects("local n: integer = 1\nn ..= \"x\"", "integer");
}

#[test]
fn compound_assignment_enforces_declared_field_types() {
    rejects(
        r#"
        class C
            label: string
            fn init()
                self.label = "a"
            end
        end
        local c: C = C()
        c.label += 5
        "#,
        "`+`",
    );
}

#[test]
fn compound_assignment_enforces_table_element_types() {
    rejects("local t: table<string> = {\"a\"}\nt[1] += 1", "`+`");
    accepts("local t: table<integer> = {1}\nt[1] += 1");
}

#[test]
fn compound_assignment_allows_operator_overloads() {
    accepts(
        r#"
        class Vec implements OpAdd<Vec, Vec>
            x: integer
            fn init(x: integer)
                self.x = x
            end
            fn add(other: Vec) -> Vec
                return Vec(self.x + other.x)
            end
        end
        local v: Vec = Vec(1)
        v += Vec(2)
        "#,
    );
}

// ── Bitwise operators ────────────────────────────────────────────────────

#[test]
fn bitwise_operators_accept_integers() {
    accepts(
        r#"
        local a: integer = 0b1100
        local b: integer = 0b1010
        local band: integer = a & b
        local bor: integer = a | b
        local bxor: integer = a ~ b
        local shl: integer = a << 2
        local shr: integer = a >> 2
        local bnot: integer = ~a
        "#,
    );
}

#[test]
fn bitwise_operators_reject_floats() {
    // Stricter than Lua 5.3, which converts a float with no fractional
    // part. Saule never mixes the two numeric kinds implicitly, and a bit
    // pattern is a property only an `integer` has.
    rejects("local f: float = 6.0\nlocal x: integer = f & 1", "`&`");
    rejects("local f: float = 6.0\nlocal x: integer = f | 1", "`|`");
    rejects("local f: float = 6.0\nlocal x: integer = f ~ 1", "`~`");
    rejects("local f: float = 6.0\nlocal x: integer = 1 << f", "`<<`");
    rejects("local f: float = 6.0\nlocal x: integer = 1 >> f", "`>>`");
    // Unary `~f` is deliberately absent: the checker validates operand
    // *kinds* for binary operators only — `-s` on a string is a runtime
    // error too — so `~` on a float is caught by the interpreter. See
    // `bitwise_complement_rejects_a_float` in the interpreter suite.
}

#[test]
fn bitwise_operators_reject_strings() {
    rejects("local s: string = \"x\"\nlocal n: integer = s & 1", "`&`");
    rejects("local s: string = \"x\"\nlocal n: integer = 1 << s", "`<<`");
}

#[test]
fn bitwise_result_is_an_integer_whatever_the_shift_count() {
    // The result follows the operator, not the operands — unlike
    // arithmetic, where `float + float` is a `float`.
    accepts("local n: integer = 1 << 3");
    rejects("local f: float = 1 << 3", "float");
    rejects("local s: string = 8 >> 1", "string");
}

#[test]
fn bitwise_compound_assignment_enforces_the_same_rules() {
    accepts("local n: integer = 1\nn &= 3\nn |= 4\nn <<= 2\nn >>= 1");
    rejects("local f: float = 1.0\nf &= 3", "`&`");
    rejects("local s: string = \"a\"\ns <<= 1", "`<<`");
}

#[test]
fn bitwise_operators_allow_operator_overloads() {
    accepts(
        r#"
        class Mask implements OpBAnd<Mask, Mask>, OpBOr<Mask, Mask>, OpBNot<Mask>
            bits: integer
            fn init(bits: integer)
                self.bits = bits
            end
            fn band(other: Mask) -> Mask
                return Mask(self.bits & other.bits)
            end
            fn bor(other: Mask) -> Mask
                return Mask(self.bits | other.bits)
            end
            fn bnot() -> Mask
                return Mask(~self.bits)
            end
        end
        local m: Mask = Mask(0b1100) & Mask(0b1010)
        local n: Mask = m | Mask(1)
        local o: Mask = ~n
        "#,
    );
}

#[test]
fn a_class_without_the_contract_cannot_be_shifted() {
    rejects(
        r#"
        class Mask
            bits: integer
            fn init(bits: integer)
                self.bits = bits
            end
        end
        local m: Mask = Mask(1)
        local n: Mask = m << 2
        "#,
        "`<<`",
    );
}

// ── OpIndex / OpNewIndex / Assignable ─────────────────────────────────────────

/// A `Str` wrapping a `string`, exposing only what it declares itself.
///
/// `string` is a type and has no members; `String` is a separate class of
/// static functions. A wrapper writes the methods it wants to expose.
fn with_str(body: &str) -> String {
    format!(
        r#"
        class Str implements Assignable<string>
            str: string
            fn init(str: string)
                self.str = str
            end
            static fn of(s: string) -> Str return Str(s) end
            fn value() -> string return self.str end
        end
        {body}
        "#
    )
}

#[test]
fn op_index_types_the_element_from_its_own_method() {
    accepts(
        r#"
        class Cfg implements OpIndex<string, integer>
            fn index(key: string) -> integer return 1 end
        end
        local c: Cfg = Cfg()
        local n: integer = c["a"]
        "#,
    );
    rejects(
        r#"
        class Cfg implements OpIndex<string, integer>
            fn index(key: string) -> integer return 1 end
        end
        local c: Cfg = Cfg()
        local s: string = c["a"]
        "#,
        "integer",
    );
}

#[test]
fn op_index_checks_the_key_type() {
    rejects(
        r#"
        class Cfg implements OpIndex<string, integer>
            fn index(key: string) -> integer return 1 end
        end
        local c: Cfg = Cfg()
        local n: integer = c[42]
        "#,
        "Cfg.index",
    );
}

#[test]
fn indexing_a_class_without_op_index_is_rejected() {
    rejects(
        r#"
        class Plain
            n: integer
            fn init()
                self.n = 1
            end
        end
        local p: Plain = Plain()
        local x: any = p["k"]
        "#,
        "`[]`",
    );
}

#[test]
fn assignable_applies_at_an_annotated_binding() {
    accepts(&with_str("local s: Str = \"hello\""));
    // …and to a nullable slot, which names the same target for a non-nil
    // value.
    accepts(&with_str("local s: Str? = \"hello\""));
}

#[test]
fn assignable_applies_to_a_function_parameter() {
    accepts(&with_str(
        "fn take(s: Str) -> string return s.value() end\nlocal out: string = take(\"hi\")",
    ));
}

#[test]
fn assignable_does_not_leak_to_sites_that_never_convert() {
    // The soundness boundary. The interpreter converts only at annotated
    // bindings and parameters, so relaxing anywhere else would typecheck a
    // value that is never built — leaving a raw `string` where a `Str` is
    // expected. Each of these must stay rejected.
    rejects(&with_str("local all: table<Str> = {\"a\"}"), "Str");
    rejects(&with_str("local s: Str = \"a\"\ns = \"b\""), "Str");
    rejects(
        r#"
        class Str implements Assignable<string>
            str: string
            fn init(str: string)
                self.str = str
            end
            static fn of(s: string) -> Str return Str(s) end
        end
        class Box
            held: Str
            fn init()
                self.held = Str("x")
            end
            fn set() self.held = "raw" end
        end
        "#,
        "Str",
    );
}

#[test]
fn assignable_needs_a_static_method() {
    // An *instance* `of` is a different thing and must not silently
    // become a conversion — the contract is explicitly static.
    rejects(
        r#"
        class Str implements Assignable<string>
            str: string
            fn init(str: string)
                self.str = str
            end
            fn of(s: string) -> Str return Str(s) end
        end
        local s: Str = "hello"
        "#,
        "Str",
    );
}

// ── Cast resolution ─────────────────────────────────────────────────────

/// Collect the [`CastKind`] of every `as` in `src`, in source order, after
/// running the resolving check.
fn cast_kinds(src: &str) -> Vec<saule_ast::CastKind> {
    let mut module = parse(src);
    let _ = saule_semantic::analyze(&module);
    let _ = saule_typeck::check_and_resolve(&mut module);

    let mut kinds = Vec::new();
    saule_ast::visit_exprs(&module, &mut |e| {
        if let saule_ast::Expr::Cast { kind, .. } = &e.value {
            kinds.push(*kind);
        }
    });
    kinds
}

/// The stamping pass is what carries the checker's decision to the two
/// engines. A cast it misses runs as the type test, which is silently the
/// wrong answer for a conversion — so "every cast got a kind" is the
/// property worth pinning, not just the kinds themselves.
#[test]
fn every_cast_is_stamped_with_the_reading_the_checker_chose() {
    use saule_ast::CastKind::{Checked, Convert};

    let kinds = cast_kinds(&in_fn(
        "  local a: any = 1\n\
        \x20 local w = a as integer\n\
        \x20 local x = 10.5 as integer\n\
        \x20 local y = \"42\" as float\n\
        \x20 local z = true as string",
    ));
    assert_eq!(kinds, vec![Checked, Convert, Convert, Convert]);
}

/// A lambda body lives behind an `Arc`, so the resolving walk has to write
/// *through* one to reach a cast inside it. Getting this wrong leaves the
/// body unstamped and the conversion silently returning `nil`.
#[test]
fn a_cast_inside_a_lambda_body_is_stamped() {
    let kinds = cast_kinds(&in_fn("  local f = (n: float) => n as integer"));
    assert_eq!(kinds, vec![saule_ast::CastKind::Convert]);
}

/// Resolution must not disturb the numbering: the type table and the
/// binding table are both keyed by `NodeId`, and the bytecode compiler
/// reads all three together.
#[test]
fn resolving_leaves_node_ids_alone() {
    let src = in_fn("  local a: any = 1\n  local n = a as integer");
    let before = parse(&src);
    let mut after = parse(&src);
    let _ = saule_semantic::analyze(&after);
    let _ = saule_typeck::check_and_resolve(&mut after);

    let ids = |m: &Module| {
        let mut v = Vec::new();
        saule_ast::visit_exprs(m, &mut |e| v.push(e.id));
        v
    };
    assert_eq!(ids(&before), ids(&after));
}

// ─── generic declarations ───────────────────────────────────────────────
//
// `enum Result<T>`, `class Box<T>` and `interface Repo<T>` all carry real
// type arguments. These used to be parsed and discarded, so `Box<integer>`
// and `Box<string>` were the same type and the argument meant nothing.

const GENERIC_BOX: &str = "\
class Box<T>
  value: T
  fn init(value: T)
    self.value = value
  end
  fn get() -> T
    return self.value
  end
end
";

const RESULT: &str = "\
enum Result<T>
  Ok(value: T),
  Err(message: string)
end
";

#[test]
fn generic_class_member_takes_the_receivers_type_argument() {
    accepts(&format!(
        "{GENERIC_BOX}{}",
        in_fn("  local b: Box<integer> = Box(1)\n  local n: integer = b.get()")
    ));
}

#[test]
fn generic_class_member_rejects_the_other_instantiations_type() {
    rejects(
        &format!(
            "{GENERIC_BOX}{}",
            in_fn("  local b: Box<integer> = Box(1)\n  local s: string = b.get()")
        ),
        "cannot assign",
    );
}

/// A field declared `T` gets the same substitution a method's return does.
#[test]
fn generic_class_field_takes_the_receivers_type_argument() {
    accepts(&format!(
        "{GENERIC_BOX}{}",
        in_fn("  local b: Box<string> = Box(\"hi\")\n  local s: string = b.value")
    ));
}

/// Instantiations are distinct types. They share one declaration, but a
/// `Box<string>` in a `Box<integer>` slot is an alias through which the
/// element type could be violated.
#[test]
fn generic_arguments_are_invariant() {
    rejects(
        &format!(
            "{GENERIC_BOX}{}",
            in_fn("  local b: Box<integer> = Box(\"no\")")
        ),
        "cannot assign",
    );
}

#[test]
fn generic_class_infers_its_argument_from_the_constructor() {
    accepts(&format!(
        "{GENERIC_BOX}{}",
        in_fn("  local b = Box(1)\n  local n: integer = b.get()")
    ));
}

#[test]
fn generic_argument_count_must_match_the_declaration() {
    rejects(
        &format!(
            "{GENERIC_BOX}{}",
            in_fn("  local b: Box<integer, string> = Box(1)")
        ),
        "expects 1 type argument",
    );
}

#[test]
fn a_non_generic_declaration_takes_no_type_arguments() {
    rejects(
        &format!(
            "class Plain\n  x: integer\n  fn init(x: integer)\n    self.x = x\n  end\nend\n{}",
            in_fn("  local p: Plain<integer> = Plain(1)")
        ),
        "is not generic",
    );
}

/// The bare name means "some instantiation, unknown which", so it is
/// accepted against any of them — in both directions.
#[test]
fn a_bare_generic_name_is_compatible_with_any_instantiation() {
    accepts(&format!(
        "{GENERIC_BOX}{}",
        in_fn("  local b: Box = Box(1)\n  local c: Box<integer> = b")
    ));
}

#[test]
fn generic_enum_binds_its_payload_at_the_type_argument() {
    accepts(&format!(
        "{RESULT}{}",
        in_fn(
            "  local r: Result<integer> = Result.Ok(1)\n  \
             local n: integer = match r\n    \
             case Result.Ok(v) then v\n    \
             case Result.Err(m) then 0\n  end"
        )
    ));
}

#[test]
fn generic_enum_payload_is_not_the_other_instantiations_type() {
    rejects(
        &format!(
            "{RESULT}{}",
            in_fn(
                "  local r: Result<integer> = Result.Ok(1)\n  \
                 local s: string = match r\n    \
                 case Result.Ok(v) then v\n    \
                 case Result.Err(m) then m\n  end"
            )
        ),
        "incompatible types",
    );
}

/// `Err` says nothing about `T`, so the construction cannot contradict the
/// annotation — and the annotation is what supplies the instantiation.
#[test]
fn a_variant_that_pins_nothing_down_fits_any_instantiation() {
    accepts(&format!(
        "{RESULT}{}",
        in_fn("  local r: Result<integer> = Result.Err(\"boom\")")
    ));
}

#[test]
fn generic_enum_still_requires_exhaustive_arms() {
    rejects(
        &format!(
            "{RESULT}{}",
            in_fn(
                "  local r: Result<integer> = Result.Ok(1)\n  \
                 local n: integer = match r\n    \
                 case Result.Ok(v) then v\n  end"
            )
        ),
        "non-exhaustive",
    );
}

/// Two parameters bind independently, from the positions they appear in.
#[test]
fn multiple_type_parameters_bind_independently() {
    accepts(&format!(
        "class Pair<A, B>\n  left: A\n  right: B\n  \
         fn init(left: A, right: B)\n    self.left = left\n    self.right = right\n  end\n  \
         fn first() -> A\n    return self.left\n  end\nend\n{}",
        in_fn("  local p = Pair(1, \"x\")\n  local n: integer = p.first()")
    ));
}

/// A type parameter is in scope for the declaration's own members — without
/// that, `class Box<T>` fails on its own field.
#[test]
fn a_type_parameter_is_in_scope_for_its_declarations_members() {
    accepts(GENERIC_BOX);
}

#[test]
fn a_generic_interface_is_implemented_at_a_concrete_instantiation() {
    accepts(
        "interface Repo<T>\n  fn find(id: integer) -> T?\nend\n\
         class IntRepo implements Repo<integer>\n  \
         local items: table<integer>\n  \
         fn init()\n    self.items = {}\n  end\n  \
         fn find(id: integer) -> integer?\n    return self.items[id]\n  end\nend\n",
    );
}
