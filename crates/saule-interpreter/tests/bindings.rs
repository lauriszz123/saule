//! Phase 0.6: the binding table `saule-semantic` now publishes.
//!
//! Two things are being pinned here.
//!
//! **Slots.** Every local gets an index into its function's frame, which is
//! what lets the compiler emit `R[3]` where the tree-walker emits a hash
//! probe up a chain of scopes.
//!
//! **Exact upvalues.** `saule-interpreter`'s `capture.rs` answers the
//! capture question by over-approximating — "every identifier the body
//! mentions" — and bails out to whole-scope capture on a nested
//! declaration. These tests assert the precise answer: a closure captures
//! what it actually references, nothing else, and a name reached across two
//! function boundaries is threaded through the middle one rather than
//! grabbed directly.

use saule_ast::{Expr, Module, NodeId, Spanned};
use saule_lexer::Lexer;
use saule_parser::parse;
use saule_semantic::{Binding, Bindings, ModuleSeed, UpvalRef, analyze_with_bindings};

/// Lives in this crate rather than in `saule-semantic` for one reason:
/// `print` and friends only resolve once the interpreter has installed the
/// prelude provider, so a test written next to the resolver cannot use the
/// standard library at all.
fn analyze(src: &str) -> (Module, Bindings) {
    saule_interpreter::init();
    let toks = Lexer::new(src).tokenize().expect("lex");
    let module = parse(toks).expect("parse");
    let (errors, bindings) = analyze_with_bindings(&module, ModuleSeed::default());
    assert!(errors.is_empty(), "semantic errors: {errors:?}");
    (module, bindings)
}

/// Bindings recorded for every `Ident` spelled `name`, in source order.
fn bindings_of(m: &Module, b: &Bindings, name: &str) -> Vec<Binding> {
    let mut out = Vec::new();
    saule_ast::visit_exprs(m, &mut |e| {
        if matches!(&e.value, Expr::Ident(n) if n == name)
            && let Some(bind) = b.get(e.id)
        {
            out.push(bind.clone());
        }
    });
    out
}

/// The node id of the nth lambda in the module, in source order.
fn nth_lambda(m: &Module, n: usize) -> NodeId {
    let mut ids = Vec::new();
    saule_ast::visit_exprs(m, &mut |e: &Spanned<Expr>| {
        if matches!(&e.value, Expr::Lambda { .. }) {
            ids.push(e.id);
        }
    });
    ids[n]
}

#[test]
fn parameters_and_locals_get_consecutive_slots() {
    let (m, b) = analyze(
        r#"
fn f(a: integer, bb: integer) -> integer
  local c: integer = a + bb
  local d: integer = c
  return d
end
"#,
    );
    assert_eq!(bindings_of(&m, &b, "a"), vec![Binding::Local { slot: 0 }]);
    assert_eq!(bindings_of(&m, &b, "bb"), vec![Binding::Local { slot: 1 }]);
    assert_eq!(bindings_of(&m, &b, "c"), vec![Binding::Local { slot: 2 }]);
    assert_eq!(bindings_of(&m, &b, "d"), vec![Binding::Local { slot: 3 }]);
}

#[test]
fn top_level_declarations_are_module_slots() {
    let (m, b) = analyze(
        r#"
local first: integer = 1
local second: integer = 2

fn use_them() -> integer
  return first + second
end
"#,
    );
    // Read from inside a function: a module slot, not an upvalue. Reaching a
    // top-level name is `GETMOD`, which needs no capture chain at all.
    assert_eq!(
        bindings_of(&m, &b, "first"),
        vec![Binding::Module { slot: 0 }]
    );
    assert_eq!(
        bindings_of(&m, &b, "second"),
        vec![Binding::Module { slot: 1 }]
    );
    assert_eq!(b.module_slots.first().map(|s| s.as_ref()), Some("first"));
    assert_eq!(b.module_slots.get(2).map(|s| s.as_ref()), Some("use_them"));
}

#[test]
fn a_closure_captures_only_what_it_references() {
    // The property `capture.rs` cannot provide: `used` is captured, `unused`
    // is not, even though both are in scope at the point the lambda is
    // written.
    let (m, b) = analyze(
        r#"
fn outer() -> nil
  local used: integer = 1
  local unused: integer = 2
  local f = fn() -> integer
    return used
  end
  print(f(), unused)
end
"#,
    );
    let info = b.function(nth_lambda(&m, 0)).expect("lambda recorded");
    assert_eq!(
        info.upval_names.iter().map(|s| s.as_ref()).collect::<Vec<_>>(),
        vec!["used"],
        "captured more than the body references"
    );
    assert_eq!(info.upvals, vec![UpvalRef::ParentLocal { slot: 0 }]);
    assert_eq!(bindings_of(&m, &b, "used"), vec![Binding::Upvalue { index: 0 }]);
}

#[test]
fn a_nested_declaration_does_not_defeat_the_analysis() {
    // `capture.rs` documents that it gives up entirely — `opaque = true` —
    // as soon as the body contains a nested `Stmt::Decl`, and falls back to
    // capturing the whole enclosing scope. That fallback is a leak. Here the
    // inner body has a nested `fn` and the answer is still exact.
    //
    // `helper` is declared but not called: a nested `fn` inside a function
    // body is not bound into the enclosing scope today, which is a separate
    // pre-existing gap. Its mere presence is what defeats `capture.rs`, and
    // that is the part under test.
    let (m, b) = analyze(
        r#"
fn outer() -> nil
  local wanted: integer = 1
  local ignored: integer = 2
  local f = fn() -> integer
    fn helper(n: integer) -> integer
      return n
    end
    return wanted
  end
  print(f(), ignored)
end
"#,
    );
    let info = b.function(nth_lambda(&m, 0)).expect("lambda recorded");
    assert_eq!(
        info.upval_names.iter().map(|s| s.as_ref()).collect::<Vec<_>>(),
        vec!["wanted"],
        "a nested declaration made the capture set over-approximate again"
    );
}

#[test]
fn capture_threads_through_intermediate_functions() {
    // Two boundaries deep. The middle closure must gain an upvalue it never
    // mentions, because the inner one reaches through it — that link is
    // exactly what a flat "names the body mentions" analysis cannot express.
    let (m, b) = analyze(
        r#"
fn outer() -> nil
  local x: integer = 1
  local mid = fn() -> nil
    local inner = fn() -> integer
      return x
    end
    print(inner())
  end
  mid()
end
"#,
    );
    let mid = b.function(nth_lambda(&m, 0)).expect("mid recorded");
    let inner = b.function(nth_lambda(&m, 1)).expect("inner recorded");

    assert_eq!(
        mid.upvals,
        vec![UpvalRef::ParentLocal { slot: 0 }],
        "middle closure did not pick up the link"
    );
    assert_eq!(
        inner.upvals,
        vec![UpvalRef::ParentUpvalue { index: 0 }],
        "inner closure should reach through the middle, not past it"
    );
}

#[test]
fn a_name_captured_twice_gets_one_upvalue() {
    let (m, b) = analyze(
        r#"
fn outer() -> nil
  local x: integer = 1
  local f = fn() -> integer
    return x + x + x
  end
  print(f())
end
"#,
    );
    let info = b.function(nth_lambda(&m, 0)).expect("lambda recorded");
    assert_eq!(info.upvals.len(), 1, "duplicate upvalue entries");
    assert_eq!(
        bindings_of(&m, &b, "x"),
        vec![Binding::Upvalue { index: 0 }; 3]
    );
}

#[test]
fn a_self_recursive_local_closure_captures_itself() {
    // `local fact = fn(n) … fact(n-1) … end` — the shape
    // `FunctionObject::self_name` and `Environment::drop_capture` exist to
    // handle. The resolver binds the name before walking the lambda, so the
    // reference resolves to an upvalue pointing at the slot the closure is
    // about to be stored in.
    let (m, b) = analyze(
        r#"
fn outer() -> nil
  local fact = fn(n: integer) -> integer
    if n <= 1 then return 1 end
    return n * fact(n - 1)
  end
  print(fact(5))
end
"#,
    );
    let info = b.function(nth_lambda(&m, 0)).expect("lambda recorded");
    assert_eq!(
        info.upval_names.iter().map(|s| s.as_ref()).collect::<Vec<_>>(),
        vec!["fact"]
    );
    assert_eq!(info.upvals, vec![UpvalRef::ParentLocal { slot: 0 }]);
}

#[test]
fn block_locals_do_not_leak_into_module_scope() {
    // The regression this rewrite nearly introduced: a `local` inside a
    // block at top level is an ordinary local of the module body, not a
    // module slot, and must not be visible after the block ends.
    let (m, b) = analyze(
        r#"
if true then
  local hidden: integer = 1
  print(hidden)
end
"#,
    );
    assert!(
        !b.module_slots.iter().any(|s| s.as_ref() == "hidden"),
        "a block-scoped local escaped into module scope"
    );
    assert_eq!(
        bindings_of(&m, &b, "hidden"),
        vec![Binding::Local { slot: 0 }]
    );
}

#[test]
fn a_closure_in_a_top_level_block_still_captures() {
    // Follows from the above: since such a local lives in the module body's
    // frame, a closure beside it captures it like any other local.
    let (m, b) = analyze(
        r#"
if true then
  local n: integer = 7
  local f = fn() -> integer
    return n
  end
  print(f())
end
"#,
    );
    let info = b.function(nth_lambda(&m, 0)).expect("lambda recorded");
    assert_eq!(info.upvals, vec![UpvalRef::ParentLocal { slot: 0 }]);
}

#[test]
fn sibling_blocks_reuse_slots() {
    // Stack discipline (§18): two blocks that cannot be live at once share
    // the register.
    let (m, b) = analyze(
        r#"
fn f(flag: boolean) -> nil
  if flag then
    local a: integer = 1
    print(a)
  else
    local bb: integer = 2
    print(bb)
  end
end
"#,
    );
    assert_eq!(bindings_of(&m, &b, "a"), vec![Binding::Local { slot: 1 }]);
    assert_eq!(bindings_of(&m, &b, "bb"), vec![Binding::Local { slot: 1 }]);
}

#[test]
fn prelude_and_self_are_distinguished() {
    let (m, b) = analyze(
        r#"
class P
  fn init()
    self.x = 1
  end
  x: integer

  fn show() -> nil
    print(self.x)
  end
end
"#,
    );
    assert!(matches!(
        bindings_of(&m, &b, "print").as_slice(),
        [Binding::Prelude { .. }]
    ));

    let mut selfs = 0;
    saule_ast::visit_exprs(&m, &mut |e| {
        if matches!(e.value, Expr::Self_) {
            assert_eq!(b.get(e.id), Some(&Binding::SelfRef));
            selfs += 1;
        }
    });
    assert_eq!(selfs, 2);
}

#[test]
fn a_class_static_read_by_bare_name_is_not_a_local() {
    let (m, b) = analyze(
        r#"
class Counter
  static total: integer = 0

  fn bump() -> nil
    total = total + 1
  end
end
"#,
    );
    let binds = bindings_of(&m, &b, "total");
    assert!(!binds.is_empty(), "no binding recorded for `total`");
    for bind in &binds {
        match bind {
            Binding::ClassStatic { class, name } => {
                assert_eq!(class.as_ref(), "Counter");
                assert_eq!(name.as_ref(), "total");
            }
            other => panic!("a static read resolved to {other:?}, which would compile to a register"),
        }
    }
}

#[test]
fn asking_for_bindings_does_not_change_diagnostics() {
    for src in [
        "local x: integer = 1",
        "print(nope)",
        "nope = 1",
        "local x: integer = x + 1",
        "fn f() -> nil\n  self.x = 1\nend",
        "if true then local y: integer = 1 end\nprint(y)",
    ] {
        let toks = Lexer::new(src).tokenize().expect("lex");
        let module = parse(toks).expect("parse");
        saule_interpreter::init();
        let plain: Vec<String> = saule_semantic::analyze_with_seed(&module, ModuleSeed::default())
            .iter()
            .map(|e| e.to_string())
            .collect();
        let (with_bindings, _) = analyze_with_bindings(&module, ModuleSeed::default());
        let collected: Vec<String> = with_bindings.iter().map(|e| e.to_string()).collect();
        assert_eq!(plain, collected, "diagnostics diverged for:\n{src}");
    }
}

#[test]
fn module_slot_order_is_deterministic() {
    // Slot numbers end up in bytecode; they must not move between runs.
    let src = "local a = 1\nlocal b = 2\nfn c() end\nclass D end";
    let first = analyze(src).1.module_slots;
    for _ in 0..5 {
        assert_eq!(analyze(src).1.module_slots, first);
    }
}
