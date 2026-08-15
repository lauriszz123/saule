//! `saule` command-line driver — dispatch only. The actual work lives in:
//!
//! * [`cli`] — the `clap` command surface
//! * [`init`] — project scaffolding
//! * [`project`] — `saule.config` parsing and project-mode bootstrap
//! * [`run`] — file execution pipeline (lex → parse → typecheck → run → `Main`)

use std::path::Path;

use clap::{CommandFactory, Parser};

use cli::{Cli, Command, RunArgs};

mod check;
mod disasm;
mod cli;
mod fmt;
mod init;
mod project;
mod run;

/// Stack reserved for the thread the toolchain actually runs on, on every
/// platform except macOS (see [`main`] for why macOS stays on the first
/// thread and how it gets its stack instead).
///
/// The interpreter is a recursive tree-walker: one Saule call costs a chain of
/// native frames, and those frames are large (they carry `Result<Vec<Value>,
/// RuntimeError>` temporaries, and `RuntimeError` is a wide enum). Measured on
/// the default 8 MiB main-thread stack, a debug build aborts at roughly 300
/// Saule frames — far too shallow for ordinary recursive code, and the abort
/// is an uncatchable `SIGABRT` rather than an error.
///
/// Reserving a large stack here is what makes
/// [`saule_interpreter::eval::MAX_EVAL_DEPTH`] the binding limit instead of
/// the OS, so deep-but-bounded recursion runs and unbounded recursion gets a
/// real diagnostic. This is address space, not committed memory — pages are
/// faulted in only as the stack actually grows. rustc does the same thing for
/// the same reason.
/// Sized against the measured worst case. With this stack a debug build —
/// the pessimistic one, since its frames are much larger than release's —
/// reaches roughly 24k Saule frames before the OS gives out, against a
/// default limit of 10k. That ~2x margin is deliberate: a frame's cost varies
/// with how much expression nesting and how many arguments each call carries,
/// so the limit has to hold for heavier frames than this measurement used.
const RUN_STACK_SIZE: usize = 1024 * 1024 * 1024;

fn main() {
    // macOS: stay on the process's first thread. AppKit refuses window and
    // menu work from anywhere else — `Window.create` on a spawned thread dies
    // in `-[NSApplication setMainMenu:]` with an uncatchable
    // `NSInternalInconsistencyException` — and minifb pumps the event queue
    // from whichever thread opened the window, so the whole interpreter has to
    // live here. The stack this thread gets is set by the linker in
    // `build.rs`, because a thread's stack cannot be resized after exec.
    if cfg!(target_os = "macos") {
        real_main();
        return;
    }

    // Everything runs on the big-stack thread; `main` only joins it and
    // forwards the exit code. Nothing below should call `std::process::exit`
    // expecting to bypass this.
    let spawned = std::thread::Builder::new()
        .name("saule-main".into())
        .stack_size(RUN_STACK_SIZE)
        .spawn(real_main);

    match spawned {
        // A panic in the child has already printed its message; propagate it
        // as a non-zero exit rather than a second panic message from `main`.
        Ok(child) => {
            if child.join().is_err() {
                std::process::exit(101);
            }
        }
        // A platform that won't give us the reservation (32-bit address
        // space, a restrictive ulimit) still runs — just with whatever the
        // main thread has. The depth guard is what keeps that safe; it simply
        // becomes the less generous of the two limits.
        Err(_) => real_main(),
    }
}

fn real_main() {
    let cli = Cli::parse();

    if cli.version {
        // The long form: a development build says so, and says which commit,
        // because "saule 26.7" in a bug report should only ever mean the
        // released 26.7.
        println!("saule {}", saule_version::FULL);
        return;
    }

    // Bare `saule`: print help and exit 0, matching the pre-clap behaviour.
    let Some(command) = cli.command else {
        let _ = Cli::command().print_help();
        println!();
        return;
    };

    match command {
        Command::Run(args) => cmd_run(args),
        Command::Check(args) => check::cmd_check(args.target, args.dump_type_coverage),
        Command::Fmt(args) => fmt::cmd_fmt(&args),
        Command::Init(args) => init::cmd_init(&args.name, args.lib),
        Command::Disasm(args) => disasm::cmd_disasm(&args.file),
    }
}

/// Route `saule run` to project or single-file mode.
///
/// The only question asked is whether the target is a directory. Nothing
/// sniffs file extensions and nothing probes for a `saule.config` to guess
/// the user's intent — script arguments have their own place, after `--`,
/// so there is nothing left to disambiguate.
fn cmd_run(args: RunArgs) {
    if args.vm {
        run::request_vm();
    }
    saule_interpreter::stdlib::os::set_script_args(args.args);

    match args.target {
        None => project::run_project(Path::new(".")),
        Some(target) if target.is_dir() => project::run_project(&target),
        Some(target) => run::run_file(target, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).expect("args should parse")
    }

    fn run_args(args: &[&str]) -> RunArgs {
        match parse(args).command {
            Some(Command::Run(r)) => r,
            other => panic!("expected a run command, got {other:?}"),
        }
    }

    #[test]
    fn bare_run_is_the_current_directory() {
        let parsed = run_args(&["saule", "run"]);
        assert_eq!(parsed.target, None);
        assert!(parsed.args.is_empty());
    }

    #[test]
    fn target_is_taken_verbatim_whatever_its_shape() {
        // No extension sniffing: a directory, a `.sau` file and an
        // extensionless path all arrive as the same plain target, and the
        // directory check happens later against the filesystem.
        for target in ["src/main.sau", "examples/bf", "somescript"] {
            let parsed = run_args(&["saule", "run", target]);
            assert_eq!(parsed.target, Some(PathBuf::from(target)));
            assert!(parsed.args.is_empty());
        }
    }

    #[test]
    fn separator_alone_means_project_mode_with_argv() {
        // The case the old extension-sniffing existed to handle: a project
        // that takes a filename of its own must not have it parsed as Saule.
        let parsed = run_args(&["saule", "run", "--", "input.bf"]);
        assert_eq!(parsed.target, None);
        assert_eq!(parsed.args, vec!["input.bf"]);
    }

    #[test]
    fn target_and_argv_can_both_be_given() {
        let parsed = run_args(&["saule", "run", "f.sau", "--", "a", "b"]);
        assert_eq!(parsed.target, Some(PathBuf::from("f.sau")));
        assert_eq!(parsed.args, vec!["a", "b"]);
    }

    #[test]
    fn script_args_may_look_like_flags() {
        let parsed = run_args(&["saule", "run", "--", "-v", "--help", "-"]);
        assert_eq!(parsed.target, None);
        assert_eq!(parsed.args, vec!["-v", "--help", "-"]);
    }

    #[test]
    fn a_second_bare_positional_is_an_error_not_a_guess() {
        // Previously this silently became "run the project, forward both
        // words". Ambiguity is now reported instead of resolved by heuristic.
        assert!(Cli::try_parse_from(["saule", "run", "a", "b"]).is_err());
    }

    #[test]
    fn version_flag_has_both_spellings() {
        for spelling in ["-v", "-V", "--version"] {
            assert!(parse(&["saule", spelling]).version, "{spelling} failed");
        }
    }

    #[test]
    fn init_requires_a_name() {
        assert!(Cli::try_parse_from(["saule", "init"]).is_err());
        match parse(&["saule", "init", "demo"]).command {
            Some(Command::Init(args)) => {
                assert_eq!(args.name, "demo");
                assert!(!args.lib, "an app is the default shape");
            }
            other => panic!("expected init, got {other:?}"),
        }
    }

    #[test]
    fn init_lib_selects_the_library_shape() {
        match parse(&["saule", "init", "demo", "--lib"]).command {
            Some(Command::Init(args)) => {
                assert_eq!(args.name, "demo");
                assert!(args.lib);
            }
            other => panic!("expected init, got {other:?}"),
        }
    }

    #[test]
    fn unknown_subcommands_are_rejected() {
        assert!(Cli::try_parse_from(["saule", "frobnicate"]).is_err());
    }

    #[test]
    fn the_command_surface_is_internally_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
