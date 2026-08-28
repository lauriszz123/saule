//! Program output is capturable in-process.
//!
//! Before `saule_interpreter::output`, `print` wrote to the process's stdout
//! through `print!`, so the only way to assert on what a program printed was
//! to spawn the CLI and read a pipe. That also meant a `wasm32-unknown-unknown`
//! build — where stdout is discarded — could run a program and show the user
//! nothing.
//!
//! These tests pin the behaviour the browser runtime depends on: install a
//! sink, run a real program through the full pipeline, get its output back.

use saule_interpreter::output::{self, Stream};

/// Run `src` through the whole pipeline with output captured.
fn run_capturing(src: &str) -> (String, Vec<(Stream, String)>) {
    let tokens = saule_lexer::Lexer::new(src).tokenize().expect("lex");
    let mut module = saule_parser::parse(tokens).expect("parse");

    let (sink, result) = output::capture(|| saule_interpreter::check_and_run(&mut module));
    result.expect("program should run cleanly");

    let chunks = sink
        .chunks()
        .into_iter()
        .map(|c| (c.stream, c.text))
        .collect();
    (sink.text(), chunks)
}

#[test]
fn captures_println_from_a_real_program() {
    let (text, _) = run_capturing(
        r#"
local name: string = "Saule"
println("hello, " .. name)
println(1 + 2)
"#,
    );
    assert_eq!(text, "hello, Saule\n3\n");
}

#[test]
fn print_does_not_add_a_newline_but_println_does() {
    let (text, _) = run_capturing(
        r#"
print("a")
print("b", "c")
println()
println("x", 1)
"#,
    );
    // `print` joins with tabs and adds nothing; `println` appends \n.
    assert_eq!(text, "ab\tc\nx\t1\n");
}

#[test]
fn captures_printf_formatting() {
    let (text, _) = run_capturing(r#"printf("%d-%s|", 42, "hi")"#);
    assert_eq!(text, "42-hi|");
}

#[test]
fn captures_output_from_inside_classes_and_loops() {
    let (text, _) = run_capturing(
        r#"
class Counter
    local n: integer

    fn init(n: integer)
        self.n = n
    end

    fn tick()
        self.n = self.n + 1
        println("n = " .. self.n)
    end
end

local c: Counter = Counter(0)

for i = 1, 3 do
    c.tick()
end
"#,
    );
    assert_eq!(text, "n = 1\nn = 2\nn = 3\n");
}

#[test]
fn separates_stdout_from_stderr() {
    let (_, chunks) = run_capturing(
        r#"
println("to stdout")
Io.stderr.write("to stderr\n")
"#,
    );
    let stdout: String = chunks
        .iter()
        .filter(|(s, _)| *s == Stream::Stdout)
        .map(|(_, t)| t.as_str())
        .collect();
    let stderr: String = chunks
        .iter()
        .filter(|(s, _)| *s == Stream::Stderr)
        .map(|(_, t)| t.as_str())
        .collect();

    assert_eq!(stdout, "to stdout\n");
    assert_eq!(stderr, "to stderr\n");
}

#[test]
fn a_program_that_prints_nothing_captures_nothing() {
    let (text, chunks) = run_capturing("local unused: integer = 1");
    assert_eq!(text, "");
    assert!(chunks.is_empty());
}
