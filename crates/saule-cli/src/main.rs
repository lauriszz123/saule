use std::{
    fs,
    path::{Path, PathBuf},
    process,
};

use miette::{NamedSource, Report};
use saule_interpreter::{Environment, Value};

const USAGE: &str = "\
Usage:
  saule run <file.sau>       run a single Saule source file
  saule run                  run the project in the current directory
  saule init <name>          scaffold a new Saule project in ./<name>
  saule --help | -h          show this help
  saule --version | -V       print the version";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let head = args.first().map(String::as_str).unwrap_or("");
    match head {
        "" | "-h" | "--help" => {
            println!("{USAGE}");
        }
        "-v" | "--version" => {
            println!("saule {}", env!("CARGO_PKG_VERSION"));
        }
        "init" => match args.get(1) {
            Some(name) => cmd_init(name),
            None => {
                eprintln!("error: `init` needs a project name\n\n{USAGE}");
                process::exit(2);
            }
        },
        "run" => match args.get(1) {
            Some(path) => run_file(PathBuf::from(path), false),
            None => run_project(&PathBuf::from(".")),
        },
        other => {
            eprintln!("Error: Unknown Command `{other}`\n\n{USAGE}");
            process::exit(2);
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// `saule init <name>` — scaffold a new project.
// ──────────────────────────────────────────────────────────────────────────────

fn cmd_init(name: &str) {
    let root = PathBuf::from(name);
    if root.exists() {
        eprintln!("error: `{name}` already exists");
        process::exit(1);
    }
    if let Err(e) = fs::create_dir_all(root.join("src")) {
        eprintln!("error creating project directory: {e}");
        process::exit(1);
    }

    let config = format!("name: \"{name}\"\nversion: \"0.1.0\"\nentry: \"src/main.sau\"\n");

    let main_sau = "\
--[[
Entry point.

The `Main` class with a `static fn main()` is the default entry point for a Saule.
]] 

class Greeter
    local who: string

    fn init(who: string)
        self.who = who
    end

    fn greet()
        println(\"hi, \" .. self.who)
    end
end

class Main
    static fn main()
        local g: Greeter = Greeter(\"world\")
        g.greet()
    end
end
";

    let gitignore = "*.log\n";

    let readme = format!("# {name}\n\nA Saule project. Run with:\n\n```sh\nsaule run\n```\n");

    let write = |relpath: &str, contents: &str| -> Result<(), std::io::Error> {
        fs::write(root.join(relpath), contents)
    };

    if let Err(e) = write("saule.config", &config)
        .and_then(|_| write("src/main.sau", main_sau))
        .and_then(|_| write(".gitignore", gitignore))
        .and_then(|_| write("README.md", &readme))
    {
        eprintln!("error writing project files: {e}");
        process::exit(1);
    }

    println!("Created project `{name}`");
    println!("  cd {name}");
    println!("  saule run");
}

// ──────────────────────────────────────────────────────────────────────────────
// `saule run` (no args) — read `saule.config` from cwd, run the file named
// by its `entry:` key. The file must declare `class Main` with a
// `static fn main()`, which is the program entry point.
// ──────────────────────────────────────────────────────────────────────────────

fn run_project(dir: &Path) {
    let config_path = dir.join("saule.config");
    if !config_path.exists() {
        eprintln!(
            "error: no `saule.config` in `{}`\n\nRun `saule init <name>` to create one, or pass a file path.",
            dir.display()
        );
        process::exit(1);
    }

    let config = match read_config(&config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error reading {}: {e}", config_path.display());
            process::exit(1);
        }
    };

    let entry = config
        .get("entry")
        .map(String::as_str)
        .unwrap_or("src/main.sau");
    run_file(dir.join(entry), true);
}

/// Parse the tiny `key: "value"` config format. Unknown keys are kept;
/// blank lines and `--` line comments are ignored.
fn read_config(path: &Path) -> std::io::Result<std::collections::HashMap<String, String>> {
    let text = fs::read_to_string(path)?;
    let mut out = std::collections::HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("--") {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_string();
        let value = value.trim().trim_matches('"').to_string();
        out.insert(key, value);
    }
    Ok(out)
}

fn run_file(path: PathBuf, require_main: bool) {
    if !path.exists() {
        eprintln!("error: file '{}' does not exist", path.display());
        process::exit(1);
    }
    let source = fs::read_to_string(&path).unwrap_or_else(|err| {
        eprintln!("error reading file '{}': {}", path.display(), err);
        process::exit(1);
    });

    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());

    if let Err(report) = run_source(&name, source, require_main) {
        eprintln!("{report:?}");
        process::exit(1);
    }
}

fn run_source(name: &str, source: String, require_main: bool) -> Result<(), Report> {
    let make_src = || NamedSource::new(name.to_string(), source.clone());

    let tokens = saule_lexer::Lexer::new(&source)
        .tokenize()
        .map_err(|e| Report::new(e).with_source_code(make_src()))?;

    let module =
        saule_parser::parse(tokens).map_err(|e| Report::new(e).with_source_code(make_src()))?;

    // Static checks run *before* evaluation so we fail fast on declarative
    // errors (missing field initializers etc.) without ever executing user
    // code. Today the checker is intentionally narrow; see `typeck`.
    let errors = saule_interpreter::typeck::check(&module);
    if let Some(first) = errors.into_iter().next() {
        return Err(Report::new(first).with_source_code(make_src()));
    }

    // Execute the file's top-level statements so declarations register.
    let env = Environment::with_prelude();
    saule_interpreter::run_in(&module, &env)
        .map_err(|e| Report::new(e).with_source_code(make_src()))?;

    // Project entry point: `class Main` with `static fn main()`.
    // When required (project mode), missing it is a hard error.
    // For single-file mode it's invoked when present as a convenience.
    let main_class = match env.borrow().get("Main") {
        Some(Value::Class(c)) => Some(c),
        _ => None,
    };
    match main_class {
        Some(c) if c.lookup_static_method("main").is_some() => {
            saule_interpreter::call_class_static_method(&c, "main", &[])
                .map_err(|e| Report::new(e).with_source_code(make_src()))?;
        }
        _ if require_main => {
            eprintln!(
                "error: `{name}` must declare `class Main` with a `static fn main()` entry point"
            );
            process::exit(1);
        }
        _ => {}
    }

    Ok(())
}
