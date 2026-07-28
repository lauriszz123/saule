use crate::{DocIndex, collect, extract, validate};
use saule_ast::Module;

fn parse(src: &str) -> Module {
    let tokens = saule_lexer::Lexer::new(src)
        .tokenize()
        .expect("source should lex");
    saule_parser::parse(tokens).expect("source should parse")
}

fn index(src: &str) -> DocIndex {
    collect(&parse(src), src)
}

/// Offset of the line starting with `needle`, for anchoring `extract`
/// without hard-coding byte counts in every test.
fn at(src: &str, needle: &str) -> usize {
    src.find(needle)
        .unwrap_or_else(|| panic!("`{needle}` not found in source"))
}

// ─── marker recognition ─────────────────────────────────────────────────

#[test]
fn three_dashes_is_a_doc_comment() {
    let src = "--- Hello.\nfn f()\nend\n";
    let d = extract(src, at(src, "fn f")).expect("doc attached");
    assert_eq!(d.summary, "Hello.");
}

#[test]
fn two_dashes_is_an_ordinary_comment() {
    let src = "-- Just a note.\nfn f()\nend\n";
    assert!(extract(src, at(src, "fn f")).is_none());
}

#[test]
fn four_or_more_dashes_is_not_a_doc_comment() {
    // The `////` escape hatch: decorative rules and banners must not
    // latch onto the declaration below them.
    for rule in ["----", "--------------------", "---- Section ----"] {
        let src = format!("{rule}\nfn f()\nend\n");
        assert!(
            extract(&src, at(&src, "fn f")).is_none(),
            "`{rule}` should be an ordinary comment"
        );
    }
}

#[test]
fn blank_line_detaches_the_block() {
    let src = "--- Orphaned.\n\nfn f()\nend\n";
    assert!(extract(src, at(src, "fn f")).is_none());
}

#[test]
fn ordinary_comment_stops_the_scan() {
    let src = "--- Not mine.\n-- plain\nfn f()\nend\n";
    assert!(extract(src, at(src, "fn f")).is_none());
}

#[test]
fn consecutive_lines_join_into_one_summary() {
    let src = "--- First.\n--- Second.\nfn f()\nend\n";
    let d = extract(src, at(src, "fn f")).unwrap();
    assert_eq!(d.summary, "First.\nSecond.");
}

#[test]
fn doc_on_the_very_first_line_of_a_file() {
    let src = "--- Top of file.\nfn f()\nend\n";
    assert_eq!(
        extract(src, at(src, "fn f")).unwrap().summary,
        "Top of file."
    );
}

#[test]
fn crlf_sources_are_handled() {
    let src = "--- Windows.\r\nfn f()\r\nend\r\n";
    assert_eq!(extract(src, at(src, "fn f")).unwrap().summary, "Windows.");
}

#[test]
fn indented_doc_lines_keep_relative_indentation() {
    // One space after the marker is consumed; the rest is Markdown.
    let src = "--- Text:\n---     code line\nfn f()\nend\n";
    let d = extract(src, at(src, "fn f")).unwrap();
    assert_eq!(d.summary, "Text:\n    code line");
}

// ─── tags ───────────────────────────────────────────────────────────────

#[test]
fn param_and_return_are_structured() {
    let src = "\
--- Adds two numbers.
--- @param a The left operand.
--- @param b The right operand.
--- @return Their sum.
fn add(a: integer, b: integer) -> integer
  return a + b
end
";
    let d = extract(src, at(src, "fn add")).unwrap();
    assert_eq!(d.summary, "Adds two numbers.");
    assert_eq!(d.param("a"), Some("The left operand."));
    assert_eq!(d.param("b"), Some("The right operand."));
    assert_eq!(d.returns.as_deref(), Some("Their sum."));
}

#[test]
fn returns_spelling_is_accepted() {
    let src = "--- D.\n--- @returns A thing.\nfn f() -> integer\n  return 1\nend\n";
    let d = extract(src, at(src, "fn f")).unwrap();
    assert_eq!(d.returns.as_deref(), Some("A thing."));
}

#[test]
fn tag_descriptions_continue_across_lines() {
    let src = "\
--- Summary.
--- @param a This description
---   wraps onto a second line.
fn f(a: integer)
end
";
    let d = extract(src, at(src, "fn f")).unwrap();
    assert_eq!(
        d.param("a"),
        Some("This description wraps onto a second line.")
    );
    assert_eq!(d.summary, "Summary.");
}

#[test]
fn unknown_tags_pass_through_verbatim() {
    // We only understand `@param` / `@return`; anything else must
    // survive into the summary rather than being silently eaten.
    let src = "--- Summary.\n--- @deprecated Use `g` instead.\nfn f()\nend\n";
    let d = extract(src, at(src, "fn f")).unwrap();
    assert_eq!(d.summary, "Summary.\n@deprecated Use `g` instead.");
}

#[test]
fn tag_prefix_is_not_mistaken_for_a_tag() {
    let src = "--- @parameterise the thing\nfn f()\nend\n";
    let d = extract(src, at(src, "fn f")).unwrap();
    assert!(d.params.is_empty());
    assert_eq!(d.summary, "@parameterise the thing");
}

#[test]
fn param_name_span_points_at_the_name() {
    let src = "--- @param width How wide.\nfn f(width: integer)\nend\n";
    let d = extract(src, at(src, "fn f")).unwrap();
    let p = &d.params[0];
    assert_eq!(&src[p.name_span.clone()], "width");
}

// ─── the shape from the design discussion ───────────────────────────────

#[test]
fn documents_a_class_its_field_and_its_initialiser() {
    let src = "\
--- A base class for all entities.
class Entity
  --- Private variable has only descriptions:
  local var: integer = 10

  --- Some other description for the initializer
  --- @param a This is a description for the parameter.
  fn init(a: string)
  end
end
";
    let idx = index(src);
    assert_eq!(
        idx.summary("Entity"),
        Some("A base class for all entities.")
    );
    assert_eq!(
        idx.summary("Entity.var"),
        Some("Private variable has only descriptions:")
    );

    let init = idx.get("Entity.init").expect("init is documented");
    assert_eq!(init.summary, "Some other description for the initializer");
    assert_eq!(
        init.param("a"),
        Some("This is a description for the parameter.")
    );
}

#[test]
fn documents_interfaces_enums_and_their_members() {
    let src = "\
--- Anything that can be drawn.
interface Drawable
  --- Render to the active surface.
  fn draw(x: integer)
end

--- Which way something faces.
enum Direction
  --- Toward the top of the screen.
  North
  --- A click at a point.
  Click(x: integer, y: integer)

  --- The opposite heading.
  fn flip() -> Direction
    return self
  end
end
";
    let idx = index(src);
    assert_eq!(idx.summary("Drawable"), Some("Anything that can be drawn."));
    assert_eq!(
        idx.summary("Drawable.draw"),
        Some("Render to the active surface.")
    );
    assert_eq!(idx.summary("Direction"), Some("Which way something faces."));
    assert_eq!(
        idx.summary("Direction.North"),
        Some("Toward the top of the screen.")
    );
    assert_eq!(idx.summary("Direction.Click"), Some("A click at a point."));
    assert_eq!(idx.summary("Direction.flip"), Some("The opposite heading."));
}

#[test]
fn modifiers_before_the_declaration_do_not_break_attachment() {
    let src = "\
--- An exported helper.
export fn helper()
end

--- A shared counter.
class C
  --- A static private field.
  static local hits: integer = 0

  --- A private method.
  local fn tick()
  end
end
";
    let idx = index(src);
    assert_eq!(idx.summary("helper"), Some("An exported helper."));
    assert_eq!(idx.summary("C.hits"), Some("A static private field."));
    assert_eq!(idx.summary("C.tick"), Some("A private method."));
}

#[test]
fn undocumented_declarations_are_absent() {
    let idx = index("fn f()\nend\n");
    assert!(idx.get("f").is_none());
    assert!(idx.is_empty());
}

#[test]
fn an_empty_doc_block_counts_as_undocumented() {
    let idx = index("---\n---\nfn f()\nend\n");
    assert!(idx.get("f").is_none());
}

// ─── validation ─────────────────────────────────────────────────────────

#[test]
fn unknown_param_is_reported() {
    let src = "--- D.\n--- @param widht How wide.\nfn f(width: integer)\nend\n";
    let warnings = validate(&parse(src), src);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].message.contains("widht"));
    assert!(warnings[0].message.contains("`width`"));
    assert_eq!(&src[warnings[0].span.clone()], "widht");
}

#[test]
fn matching_params_are_silent() {
    let src = "--- D.\n--- @param width How wide.\nfn f(width: integer)\nend\n";
    assert!(validate(&parse(src), src).is_empty());
}

#[test]
fn undocumented_params_are_not_reported() {
    // Deliberately one-directional: missing docs are not an error.
    let src = "--- D.\n--- @param a First.\nfn f(a: integer, b: integer)\nend\n";
    assert!(validate(&parse(src), src).is_empty());
}

#[test]
fn param_on_a_declaration_without_parameters() {
    let src = "--- D.\n--- @param a Nope.\nclass C\nend\n";
    let warnings = validate(&parse(src), src);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].message.contains("takes no parameters"));
}

#[test]
fn tuple_variant_fields_are_valid_param_targets() {
    let src = "\
--- E.
enum E
  --- A click.
  --- @param x Horizontal position.
  Click(x: integer, y: integer)
end
";
    assert!(validate(&parse(src), src).is_empty());
}

// ─── rendering ──────────────────────────────────────────────────────────

#[test]
fn markdown_omits_empty_sections() {
    let src = "--- Just a summary.\nfn f()\nend\n";
    let d = extract(src, at(src, "fn f")).unwrap();
    assert_eq!(d.to_markdown(), "Just a summary.");
}

#[test]
fn markdown_renders_params_and_returns() {
    let src = "\
--- Adds two numbers.
--- @param a Left.
--- @param b Right.
--- @return The sum.
fn add(a: integer, b: integer) -> integer
  return a + b
end
";
    let d = extract(src, at(src, "fn add")).unwrap();
    assert_eq!(
        d.to_markdown(),
        "Adds two numbers.\n\n- `a` — Left.\n- `b` — Right.\n\n**Returns** — The sum."
    );
}
