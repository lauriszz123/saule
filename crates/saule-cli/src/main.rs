//! `saule` command-line driver — dispatch only. The actual work lives in:
//!
//! * [`cli`] — the `clap` command surface
//! * [`init`] — project scaffolding
//! * [`project`] — `saule.config` parsing and project-mode bootstrap
//! * [`run`] — file execution pipeline (lex → parse → typecheck → run → `Main`)

use std::path::Path;

use clap::{CommandFactory, Parser};

use cli::{Cli, Command, RunArgs};

mod cli;
mod fmt;
mod init;
mod project;
mod run;

fn main() {
    let cli = Cli::parse();

    if cli.version {
        println!(
            "Saule programming language version: {}",
            env!("CARGO_PKG_VERSION")
        );
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
        Command::Fmt(args) => fmt::cmd_fmt(&args),
        Command::Init(args) => init::cmd_init(&args.name, args.lib),
    }
}

/// Route `saule run` to project or single-file mode.
///
/// The only question asked is whether the target is a directory. Nothing
/// sniffs file extensions and nothing probes for a `saule.config` to guess
/// the user's intent — script arguments have their own place, after `--`,
/// so there is nothing left to disambiguate.
fn cmd_run(args: RunArgs) {
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
