//! `cargo run -p saule-vm --release --example compare [file.sau]`
//!
//! Runs a program under both engines, checks they agree, and times them.
//! This is the measurement §21.3 gates Phase 2 on: `loop_arith` and `fib`
//! must be **at least 2.5× faster** than the tree-walker, or the design's
//! assumptions need revisiting before Phase 3.
//!
//! With no argument it runs built-in programs shaped like the benchmarks in
//! `benchmarks/sau/`, minus the `class Main` wrapper those use — classes are
//! Phase 3, so the benchmark files themselves cannot be compiled yet.

use std::rc::Rc;
use std::time::Instant;

use saule_interpreter::{Environment, Value};

const PROGRAMS: &[(&str, &str)] = &[
    (
        "loop_arith",
        "local total: integer = 0
for i = 1, 3000000 do
  total = total + i * 2 - 1
end
total",
    ),
    (
        "fib",
        "fn fib(n: integer) -> integer
  if n < 2 then return n end
  return fib(n - 1) + fib(n - 2)
end
fib(27)",
    ),
    (
        "while_sum",
        "local i: integer = 0
local total: integer = 0
while i < 3000000 do
  i = i + 1
  total = total + i
end
total",
    ),
    (
        "call_heavy",
        "fn add(a: integer, b: integer) -> integer
  return a + b
end
local total: integer = 0
for i = 1, 1000000 do
  total = add(total, i)
end
total",
    ),
];

fn main() {
    saule_interpreter::init();

    let programs: Vec<(String, String)> = match std::env::args().nth(1) {
        Some(path) => {
            let src = std::fs::read_to_string(&path).expect("read source");
            vec![(path, src)]
        }
        None => PROGRAMS
            .iter()
            .map(|(n, s)| (n.to_string(), s.to_string()))
            .collect(),
    };

    println!("{:<14} {:>12} {:>12} {:>10}", "program", "tree-walker", "vm", "speedup");
    for (name, src) in &programs {
        let toks = saule_lexer::Lexer::new(src).tokenize().expect("lex");
        let module = saule_parser::parse(toks).expect("parse");
        let errs =
            saule_interpreter::analyze_and_prepare(&module, saule_semantic::ModuleSeed::default());
        assert!(errs.is_empty(), "{name}: semantic errors {errs:?}");
        let terrs = saule_interpreter::typeck::check(&module);
        assert!(terrs.is_empty(), "{name}: type errors {terrs:?}");

        let chunk = match saule_vm::compile(&module, name, src) {
            Ok(c) => Rc::new(c),
            Err(e) => {
                println!("{name:<14} {:>12}", format!("skipped: {e}"));
                continue;
            }
        };

        // Correctness before speed: a faster wrong answer is worthless.
        let started = Instant::now();
        let walked = saule_interpreter::run_in(&module, &Environment::with_prelude())
            .expect("tree-walker ran");
        let walk_time = started.elapsed();

        let started = Instant::now();
        let ran = saule_vm::run_chunk(Rc::clone(&chunk)).expect("vm ran");
        let vm_time = started.elapsed();

        let got = ran.first().cloned().unwrap_or(Value::Nil);
        assert_eq!(
            (walked.type_name(), walked.to_display_string()),
            (got.type_name(), got.to_display_string()),
            "{name}: engines disagreed"
        );

        let speedup = walk_time.as_secs_f64() / vm_time.as_secs_f64();
        println!(
            "{name:<14} {:>10.1?} {:>10.1?} {:>9.2}x",
            walk_time, vm_time, speedup
        );
    }
}
