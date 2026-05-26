use std::{fs, path::PathBuf, process};

use miette::{NamedSource, Report};
use saule_interpreter::{Environment, Value};

fn main() {
    let args: Vec<String> = std::env::args().collect();

    match args.as_slice() {
        [_, path] => {
            let path = PathBuf::from(path);
            if !path.exists() {
                eprintln!("error: file '{}' does not exist", path.display());
                process::exit(1);
            }
            run_file(path);
        }
        [name, ..] => {
            eprintln!("usage: {} <path>", name);
            process::exit(1);
        }
        _ => {
            eprintln!("usage: saule <path>");
            process::exit(1);
        }
    }
}

fn run_file(path: PathBuf) {
    let source = fs::read_to_string(&path).unwrap_or_else(|err| {
        eprintln!("error reading file '{}': {}", path.display(), err);
        process::exit(1);
    });

    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());

    if let Err(report) = run_source(&name, source) {
        eprintln!("{report:?}");
        process::exit(1);
    }
}

fn run_source(name: &str, source: String) -> Result<(), Report> {
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

    // Convention: if the file declares `class Main` with a `static fn main`,
    // invoke it automatically. Files that just run top-level statements
    // (scripts) are unaffected.
    if let Some(Value::Class(class)) = env.borrow().get("Main")
        && class.lookup_static_method("main").is_some()
    {
        saule_interpreter::call_class_static_method(&class, "main", &[])
            .map_err(|e| Report::new(e).with_source_code(make_src()))?;
    }

    Ok(())
}
