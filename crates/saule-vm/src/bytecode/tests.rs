use std::path::PathBuf;

use super::{Source, decode, encode, origin};

/// libtest's 2 MiB thread overflows on a debug-build compile; the
/// differential harness has the same helper for the same reason.
fn on_a_real_stack(body: impl FnOnce() + Send + 'static) {
    const STACK: usize = 16 << 20;
    std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(move || {
            saule_runtime::call::set_stack_budget(STACK);
            body();
        })
        .expect("spawn")
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
}

/// Compile `files` (name, source) as a program whose entry is the first,
/// with origins recorded so it can be encoded.
fn compile(test: &str, files: &[(&str, &str)]) -> crate::program::Program {
    let dir = std::env::temp_dir().join(format!("saule-bytecode-{test}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (name, source) in files {
        std::fs::write(dir.join(name), source).unwrap();
    }
    saule_runtime::init();
    origin::enable();
    let entry: PathBuf = dir.join(files[0].0);
    let program = crate::program::compile(&entry).expect("the test program compiles");
    let _ = std::fs::remove_dir_all(&dir);
    program
}

fn same(label: &str) -> String {
    label.to_string()
}

fn run(program: crate::program::Program) -> String {
    let (out, result) = saule_runtime::output::capture(|| crate::run_program(program));
    result.expect("the program runs");
    out.text()
}

const PROGRAM: &str = r#"
from shapes import Shape, Circle, area

enum Mode
    Fast = 2
    Slow
end

class Main
    static fn main()
        local shapes: Shape[] = { Circle(2.0), Shape("square") }
        for s in shapes do
            println(s.describe() .. " " .. tostring(area(s)))
        end
        println(String.upper("folded") .. " " .. tostring(Math.floor(2.7)))
        println(tostring(Mode.Fast.value) .. " " .. tostring(IoMode.Read == IoMode.Read))
        local ok = ""
        try
            throw "boom"
        catch e: string
            ok = "caught " .. e
        end
        println(ok)
    end
end
"#;

const SHAPES: &str = r#"
export class Shape
    name: string

    fn init(name: string)
        self.name = name
    end

    fn describe() -> string
        return self.name
    end

    fn area() -> float
        return 1.0
    end
end

export class Circle extends Shape
    r: float

    fn init(r: float)
        self.super("circle")
        self.r = r
    end

    fn area() -> float
        return self.r * self.r * 3.0
    end
end

export fn area(s: Shape) -> float
    return s.area()
end
"#;

#[test]
fn a_decoded_program_runs_like_the_original() {
    on_a_real_stack(|| {
        let files = [("main.sau", PROGRAM), ("shapes.sau", SHAPES)];
        let expected = run(compile("original", &files));

        let bytes = encode(&compile("encoded", &files), Source::Lines, &same).expect("encodes");
        let decoded = decode(&bytes).expect("decodes");

        assert_eq!(run(decoded), expected);
    });
}

#[test]
fn encoding_is_deterministic_and_survives_a_round_trip() {
    on_a_real_stack(|| {
        let files = [("main.sau", PROGRAM), ("shapes.sau", SHAPES)];
        let program = compile("determinism", &files);

        let first = encode(&program, Source::Full, &same).expect("encodes");
        let again = encode(&program, Source::Full, &same).expect("encodes");
        assert_eq!(first, again);

        let reencoded =
            encode(&decode(&first).expect("decodes"), Source::Full, &same).expect("encodes");
        assert_eq!(first, reencoded);
    });
}

#[test]
fn without_source_the_text_is_gone_but_the_lines_stay() {
    on_a_real_stack(|| {
        let program = compile(
            "lines",
            &[(
                "main.sau",
                "class Main\n    static fn main()\n        println(\"secret\")\n    end\nend\n",
            )],
        );
        let bytes = encode(&program, Source::Lines, &same).expect("encodes");
        assert!(
            !bytes.windows(10).any(|w| w == b"class Main"),
            "the source text must not be in the file"
        );

        let decoded = decode(&bytes).expect("decodes");
        let text = decoded.modules[decoded.entry].source.inner().clone();
        assert_eq!(text.lines().count(), 5);
        assert_eq!(text.lines().nth(2).map(str::len), Some(25));
    });
}

#[test]
fn a_damaged_file_is_refused() {
    on_a_real_stack(|| {
        let program = compile("damaged", &[("main.sau", PROGRAM), ("shapes.sau", SHAPES)]);
        let bytes = encode(&program, Source::Lines, &same).expect("encodes");

        assert!(decode(&bytes[..bytes.len() / 2]).is_err());
        assert!(decode(b"not bytecode at all").is_err());

        let mut wrong_version = bytes.clone();
        wrong_version[8] ^= 0xff;
        assert!(decode(&wrong_version).is_err());
    });
}
