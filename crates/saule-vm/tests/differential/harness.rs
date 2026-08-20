//! Shared harness for the differential suite: run one program under both
//! engines and compare.
//!
//! Every test file in this directory is a caller of [`must_agree`] or
//! [`agree`]; nothing else belongs here.

use std::rc::Rc;

use saule_interpreter::{Environment, Value};
use saule_lexer::Lexer;
use saule_parser::parse;

/// Outcome of running one program under one engine.
#[derive(Debug, PartialEq)]
pub(crate) enum Outcome {
    Value(String),
    Error(String),
}

pub(crate) fn describe(v: &Value) -> String {
    // Includes the type name, so `1` and `1.0` — which print differently but
    // could be confused — cannot compare equal by accident.
    format!("{}:{}", v.type_name(), v.to_display_string())
}

pub(crate) fn front_end(src: &str) -> saule_ast::Module {
    saule_interpreter::init();
    let toks = Lexer::new(src).tokenize().expect("lex");
    let module = parse(toks).expect("parse");
    let errs = saule_interpreter::analyze_and_prepare(&module, saule_semantic::ModuleSeed::default());
    assert!(errs.is_empty(), "semantic errors in test source: {errs:?}");
    let terrs = saule_interpreter::typeck::check(&module);
    assert!(terrs.is_empty(), "type errors in test source: {terrs:?}");
    module
}

pub(crate) fn tree_walker(module: &saule_ast::Module) -> Outcome {
    let env = Environment::with_prelude();
    match saule_interpreter::run_in(module, &env) {
        Ok(v) => Outcome::Value(describe(&v)),
        Err(e) => Outcome::Error(e.to_string()),
    }
}

/// `None` when the compiler does not support the program yet.
pub(crate) fn vm(module: &saule_ast::Module, src: &str) -> Option<Outcome> {
    let chunk = match saule_vm::compile(module, "diff.sau", src) {
        Ok(c) => c,
        Err(saule_vm::CompileError::Unsupported { .. }) => return None,
        Err(e) => return Some(Outcome::Error(format!("compile error: {e}"))),
    };
    Some(match saule_vm::run_chunk(Rc::new(chunk)) {
        Ok(vs) => Outcome::Value(
            vs.first()
                .map(describe)
                .unwrap_or_else(|| describe(&Value::Nil)),
        ),
        Err(e) => Outcome::Error(e.to_string()),
    })
}

/// Run `body` on a thread with a **real** stack, then join.
///
/// libtest gives each test thread 2 MiB. The tree-walker spends a lot of
/// Rust frames per Saule frame, and in a debug build a program that recurses
/// even a few dozen levels deep overflows that — which aborts the whole
/// process with `STATUS_STACK_OVERFLOW`, taking every other test in the
/// binary with it and reporting nothing about which one did it.
///
/// 16 MiB rather than "enough": the point is to stop measuring libtest's
/// stack. Users run on a main thread (8 MiB on Windows, 8 MiB by default on
/// Linux), so this is the *closer* configuration, not a more forgiving one.
/// `RUST_MIN_STACK` would do the same job but only if every contributor
/// remembers to set it, which is not a property a test can rely on.
pub(crate) fn on_a_real_stack(body: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(body)
        .expect("spawn")
        .join()
        // The child already printed its own panic message; resuming it here
        // is what makes the failure land on *this* test rather than on the
        // process.
        .unwrap_or_else(|e| std::panic::resume_unwind(e));
}

/// Run under both engines and require agreement. Returns `false` when the
/// program is not compilable yet, so a caller can count coverage.
#[must_use]
pub(crate) fn agree(src: &str) -> bool {
    let module = front_end(src);
    let expected = tree_walker(&module);
    match vm(&module, src) {
        None => false,
        Some(got) => {
            assert_eq!(
                got, expected,
                "engines disagreed\n--- source ---{src}\n--- disassembly ---\n{}",
                saule_vm::compile(&module, "diff.sau", src)
                    .map(|c| saule_vm::disasm::chunk(&c))
                    .unwrap_or_default()
            );
            true
        }
    }
}

/// Assert agreement *and* that the VM actually compiled it — for cases the
/// compiler is expected to handle, so a regression to `Unsupported` fails
/// rather than silently skipping.
pub(crate) fn must_agree(src: &str) {
    if !agree(src) {
        let module = front_end(src);
        let why = saule_vm::compile(&module, "diff.sau", src)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_else(|| "compiled fine on the retry?".into());
        panic!("the compiler refused a program it should handle:
{src}
  -> {why}");
    }
}

/// A pair of classes implementing one interface — the fixture every
/// interface-dispatch and dynamic-receiver test builds on.
pub(crate) const SHAPES: &str = "interface Shape\n  fn area() -> integer\n  fn name() -> string\nend\n\
class Square implements Shape\n\
\x20 fn init(s: integer)\n    self.side = s\n  end\n  side: integer\n\
\x20 fn area() -> integer\n    return self.side * self.side\n  end\n\
\x20 fn name() -> string\n    return \"square\"\n  end\n\
end\n\
class Rect implements Shape\n\
\x20 fn init(w: integer, h: integer)\n    self.w = w\n    self.h = h\n  end\n\
\x20 w: integer\n  h: integer\n\
\x20 fn area() -> integer\n    return self.w * self.h\n  end\n\
\x20 fn name() -> string\n    return \"rect\"\n  end\n\
end\n";

/// The disassembly of a program the compiler is expected to accept.
///
/// Used where the question is *what the compiler emitted* rather than what
/// the program computes. For a tail call that is the sharper assertion: a
/// negative case ("this must **not** be one") could be shown by recursing
/// past the depth guard, but that costs ~10 000 native frames per run and
/// makes the suite depend on `RUST_MIN_STACK`. Reading the opcode back is
/// exact, cheap, and fails for the right reason.
pub(crate) fn disasm_of(src: &str) -> String {
    let module = front_end(src);
    saule_vm::compile(&module, "diff.sau", src)
        .map(|c| saule_vm::disasm::chunk(&c))
        .expect("the compiler should handle this program")
}
