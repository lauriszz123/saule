//! Shared harness for the conformance suite: run one program on the VM and
//! compare its outcome with the one recorded for it.
//!
//! Every test file in this directory is a caller of [`must_agree`] or
//! [`agree`]; nothing else belongs here.
//!
//! ## Where the expected outcomes come from
//!
//! `expected.txt`, one line per program: an FNV-1a hash of its source, then
//! its outcome. They were recorded from the tree-walking interpreter — the
//! engine that defined the language until it was removed — in the last
//! commit that had both, by running this suite with the two engines side by
//! side; every one of them was also the VM's answer. So each assertion here
//! still checks the VM against what the language has always meant, even
//! though the engine that produced the answer is gone.
//!
//! A new test's program has no recorded outcome, and fails saying so. Run
//! the suite with `SAULE_BLESS=1` to append the VM's outcome for it, then
//! read the new lines in the diff: from then on they are the specification,
//! so they deserve the review any other expected value gets.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;

use saule_lexer::Lexer;
use saule_parser::parse;
use saule_runtime::Value;

/// Outcome of running one program.
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
    saule_runtime::init();
    let toks = Lexer::new(src).tokenize().expect("lex");
    let module = parse(toks).expect("parse");
    let (errs, _) =
        saule_runtime::analyze_with_bindings(&module, saule_semantic::ModuleSeed::default());
    assert!(errs.is_empty(), "semantic errors in test source: {errs:?}");
    let terrs = saule_runtime::typeck::check(&module);
    assert!(terrs.is_empty(), "type errors in test source: {terrs:?}");
    module
}

/// `None` when the compiler refuses the program.
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

/// FNV-1a over the program text: a stable key for an expected outcome.
pub(crate) fn fnv1a(src: &str) -> u64 {
    src.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)
    })
}

/// One outcome on one line: `V ` or `E ` and the text, with `\`, newline
/// and tab escaped.
fn escape(o: &Outcome) -> String {
    let (tag, text) = match o {
        Outcome::Value(s) => ("V", s),
        Outcome::Error(s) => ("E", s),
    };
    let mut out = String::with_capacity(text.len() + 2);
    out.push_str(tag);
    out.push(' ');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

/// The inverse of [`escape`].
fn unescape(line: &str) -> Option<Outcome> {
    let (tag, text) = line.split_at_checked(2)?;
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            '\\' => out.push('\\'),
            'n' => out.push('\n'),
            't' => out.push('\t'),
            _ => return None,
        }
    }
    match tag {
        "V " => Some(Outcome::Value(out)),
        "E " => Some(Outcome::Error(out)),
        _ => None,
    }
}

const EXPECTED_FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/differential/expected.txt");

/// Every recorded outcome, by program hash.
fn recorded() -> &'static HashMap<u64, String> {
    static TABLE: OnceLock<HashMap<u64, String>> = OnceLock::new();
    TABLE.get_or_init(|| {
        include_str!("expected.txt")
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| {
                let (hash, outcome) = l.split_once('\t').expect("`hash<TAB>outcome`");
                let hash = u64::from_str_radix(hash, 16).expect("hex hash");
                (hash, outcome.to_string())
            })
            .collect()
    })
}

/// The outcome recorded for `src`. With `SAULE_BLESS` set, a program with
/// none has `got` recorded for it instead — see the module docs.
fn expected(src: &str, got: &Outcome) -> Outcome {
    let hash = fnv1a(src);
    if let Some(line) = recorded().get(&hash) {
        return unescape(line).expect("a well-formed line in expected.txt");
    }
    if std::env::var_os("SAULE_BLESS").is_some() {
        use std::io::Write;
        let line = format!("{hash:016x}\t{}\n", escape(got));
        std::fs::OpenOptions::new()
            .append(true)
            .open(EXPECTED_FILE)
            .and_then(|mut f| f.write_all(line.as_bytes()))
            .expect("append to expected.txt");
        return unescape(&escape(got)).expect("round-trip");
    }
    panic!(
        "no expected outcome recorded for this program — run with `SAULE_BLESS=1` to record \
         the VM's, and review it in the diff:\n{src}\n  VM: {got:?}"
    );
}

/// Run `body` on a thread with a **real** stack, then join.
///
/// libtest gives each test thread 2 MiB, and a program that recurses through
/// natives (a sort comparator that sorts) spends several Rust frames per
/// level; in a debug build that overflows quickly, which aborts the whole
/// process and takes every other test in the binary with it.
///
/// 16 MiB rather than "enough": the point is to stop measuring libtest's
/// stack. Users run on a main thread (8 MiB on Windows, 8 MiB by default on
/// Linux), so this is the *closer* configuration, not a more forgiving one.
/// `RUST_MIN_STACK` would do the same job but only if every contributor
/// remembers to set it, which is not a property a test can rely on.
pub(crate) fn on_a_real_stack(body: impl FnOnce() + Send + 'static) {
    const STACK: usize = 16 << 20;
    std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(move || {
            // The same rule the CLI follows: whoever sizes the thread tells
            // the runtime, so nesting that outruns it reports rather than
            // aborting the whole test binary.
            saule_runtime::call::set_stack_budget(STACK);
            body();
        })
        .expect("spawn")
        .join()
        // The child already printed its own panic message; resuming it here
        // is what makes the failure land on *this* test rather than on the
        // process.
        .unwrap_or_else(|e| std::panic::resume_unwind(e));
}

/// Run on the VM and require the recorded outcome. Returns `false` when the
/// compiler refuses the program.
#[must_use]
pub(crate) fn agree(src: &str) -> bool {
    let module = front_end(src);
    match vm(&module, src) {
        None => false,
        Some(got) => {
            let expected = expected(src, &got);
            assert_eq!(
                got, expected,
                "the VM disagreed with the recorded outcome\n--- source ---{src}\n--- disassembly ---\n{}",
                saule_vm::compile(&module, "diff.sau", src)
                    .map(|c| saule_vm::disasm::chunk(&c))
                    .unwrap_or_default()
            );
            true
        }
    }
}

/// Assert the recorded outcome *and* that the VM actually compiled it, so a
/// regression to a refusal fails rather than silently skipping.
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
