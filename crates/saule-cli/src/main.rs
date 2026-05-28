//! `saule` command-line driver — arg parsing and dispatch only. The actual
//! work lives in:
//!
//! * [`init`] — project scaffolding
//! * [`project`] — `saule.config` parsing and project-mode bootstrap
//! * [`run`] — file execution pipeline (lex → parse → typecheck → run → `Main`)

use std::{path::PathBuf, process};

mod init;
mod project;
mod run;

const USAGE: &str = "\
Usage:
  saule run <file.sau> [args...]   run a single Saule source file
  saule run [args...]              run the project in the current directory
  saule run -- [args...]           force project mode, forward args to Os.args()
  saule init <name>                scaffold a new Saule project in ./<name>
  saule --help | -h                show this help
  saule --version | -V             print the version

Anything after the file path (or after `--` in project mode) is exposed to
the script via `Os.args()`. In a project directory (one with a saule.config),
args that don't look like a file path are forwarded automatically.";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let head = args.first().map(String::as_str).unwrap_or("");
    match head {
        "" | "-h" | "--help" => {
            println!("{USAGE}");
        }
        "-v" | "--version" => {
            println!(
                "Saule programming language version: {}",
                env!("CARGO_PKG_VERSION")
            );
        }
        "init" => match args.get(1) {
            Some(name) => init::cmd_init(name),
            None => {
                eprintln!("error: `init` needs a project name\n\n{USAGE}");
                process::exit(2);
            }
        },
        "run" => {
            // Split `run` args at the first `--`: anything before is for the
            // CLI (file path or nothing), anything after is forwarded
            // verbatim as script argv.
            let run_args: Vec<String> = args.iter().skip(1).cloned().collect();
            let (cli_part, script_after_sep) = match run_args.iter().position(|a| a == "--") {
                Some(i) => (run_args[..i].to_vec(), Some(run_args[i + 1..].to_vec())),
                None => (run_args, None),
            };

            // Decide between project mode and single-file mode:
            //   * `saule run`                       → project
            //   * `saule run -- a b c`              → project, argv = a b c
            //   * `saule run file.sau …`            → single file
            //   * `saule run thing …` where `thing` isn't a `.sau`/`.saule`
            //     file AND cwd has `saule.config`   → project, argv = thing …
            //     (covers things like `saule run input.bf`, where the file
            //     should be forwarded to the script, not parsed as Saule.)
            //   * otherwise                         → single file
            let has_config = PathBuf::from("saule.config").is_file();
            let first_is_saule_file = cli_part
                .first()
                .is_some_and(|s| s.ends_with(".sau") || s.ends_with(".saule"));

            if cli_part.is_empty() || (has_config && !first_is_saule_file) {
                // Project mode. Script argv = explicit `-- …` part if given,
                // otherwise everything we accumulated before the (absent) `--`.
                let argv = script_after_sep.unwrap_or(cli_part);
                saule_interpreter::stdlib::os::set_script_args(argv);
                project::run_project(&PathBuf::from("."));
            } else {
                // Single-file mode. First non-`--` arg is the file path,
                // everything else (whether before or after `--`) is script argv.
                let mut iter = cli_part.into_iter();
                let path = iter.next().expect("cli_part non-empty checked above");
                let mut argv: Vec<String> = iter.collect();
                if let Some(extra) = script_after_sep {
                    argv.extend(extra);
                }
                saule_interpreter::stdlib::os::set_script_args(argv);
                run::run_file(PathBuf::from(path), false);
            }
        }
        other => {
            eprintln!("Error: Unknown Command `{other}`\n\n{USAGE}");
            process::exit(2);
        }
    }
}
