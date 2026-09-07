//! Command-line surface: the `clap` definitions and nothing else. Each
//! subcommand's actual work lives in its own module ([`crate::run`],
//! [`crate::fmt`], [`crate::init`], [`crate::project`]).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::fmt::FmtArgs;

/// Parse a path argument, undoing Windows' argument-escaping quirk.
///
/// `saule run ".\examples\UI Project\"` arrives at the program as
/// `.\examples\UI Project"` — with a quote the user never typed and the
/// directory separator gone. The shell quotes the argument because it
/// contains a space, and the trailing `\` then escapes the closing `"` under
/// the MSVC command-line rules Rust's argument parsing follows. Tab
/// completion produces exactly this shape, so it is the *normal* way to name
/// a directory whose path contains a space, not a user mistake.
///
/// Stripping the quote is unambiguous rather than a guess: Win32 reserves `"`
/// along with `* : < > ? \ /`, so it cannot appear anywhere in a legal
/// Windows path, and one that reaches here was never part of the name. Only
/// a **trailing** quote is removed, and only on Windows — on Unix `"` is an
/// ordinary filename character and stripping it would corrupt real paths.
///
/// A `value_parser` rather than a fix-up at the call sites, because there
/// are four of them and a fifth would forget.
pub(crate) fn path_arg(raw: &str) -> Result<PathBuf, String> {
    let cleaned = if cfg!(windows) {
        raw.strip_suffix('"').unwrap_or(raw)
    } else {
        raw
    };
    Ok(PathBuf::from(cleaned))
}

#[derive(Debug, Parser)]
#[command(
    name = "saule",
    about = "The Saule programming language",
    // The version string is printed by hand in `main` so it keeps its
    // long-standing wording rather than clap's `saule <version>` default.
    disable_version_flag = true,
    // Bare `saule` prints help and exits 0, as it always has. clap's
    // `arg_required_else_help` would exit 2 instead.
    subcommand_required = false
)]
pub(crate) struct Cli {
    /// Print the version and exit.
    #[arg(short = 'v', short_alias = 'V', long)]
    pub version: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    Run(RunArgs),
    Check(CheckArgs),
    Fmt(FmtArgs),
    Init(InitArgs),
    Disasm(DisasmArgs),
}

/// `saule disasm <file.sau>`
#[derive(Debug, Args)]
#[command(
    about = "Compile a source file to bytecode and print the disassembly",
    long_about = "Compile a source file to bytecode and print the disassembly.

  saule disasm <file.sau>

A debugging aid for the bytecode compiler, not a user-facing format: the
listing shows one line per instruction with registers, constants and
resolved jump targets.

The compiler is under construction. Anything it cannot compile yet is
reported by name rather than silently omitted, so the output is either a
complete chunk or an explicit gap."
)]
pub(crate) struct DisasmArgs {
    /// The `.sau` file to compile.
    #[arg(value_name = "FILE", value_parser = path_arg)]
    pub file: PathBuf,
}

/// `saule check [TARGET]`
///
/// Mirrors `run`'s target rules — directory (or absent) means project mode,
/// a file means that file alone — so the two commands never disagree about
/// what they are looking at.
#[derive(Debug, Args)]
#[command(
    about = "Type-check a project or a source file without running it",
    long_about = "\
Type-check a project or a source file without running it.

  saule check                the project in the current directory
  saule check <dir>          the project rooted at <dir>
  saule check <file.sau>     that file, on its own

Reports every diagnostic rather than stopping at the first, and in project
mode checks every `.sau` file under `src_dirs` — not only what the entry
point imports. Exits non-zero when anything is reported, so it can gate CI.
Libraries (`kind: \"library\"`) have no entry point and check normally."
)]
pub(crate) struct CheckArgs {
    /// Project directory or `.sau` file. Defaults to the current directory.
    #[arg(value_name = "TARGET", value_parser = path_arg)]
    pub target: Option<PathBuf>,

    /// Report how much of each file the type checker proved a type for.
    ///
    /// The bytecode compiler picks a typed opcode only where a type is
    /// known, so this is the measurement `VM_DESIGN.md` §24.1 asks for
    /// before that work is depended on. Diagnostics are unaffected.
    #[arg(long)]
    pub dump_type_coverage: bool,
}

/// `saule run [TARGET] [ARGS...] [-- ARGS...]`
///
/// Exactly one thing decides project mode from single-file mode: whether
/// `TARGET` is a directory. The first positional is that target and every
/// one after it is script argv. Everything after `--` is script argv too
/// and is never interpreted by the CLI, which is what lets a project take
/// a filename of its own (`saule run -- input.bf`) without the CLI trying
/// to parse it as the target.
#[derive(Debug, Args)]
#[command(
    about = "Run a project or a single source file",
    long_about = "\
Run a project or a single source file.

  saule run                  the project in the current directory
  saule run <dir>            the project rooted at <dir>
  saule run <file.sau>       that file, on its own
  saule run <file> a b       that file, with Os.args() = [\"a\", \"b\"]
  saule run -- a b           the current project, with Os.args() = [\"a\", \"b\"]
  saule run <file> -- -v     that file, with a script argument starting `-`

TARGET picks project mode when it is a directory (or absent) and single-file
mode when it is a file. The first positional is the target; every positional
after it is script argv. Use `--` to give argv to the project in the current
directory, or to pass an argument that starts with `-`."
)]
pub(crate) struct RunArgs {
    /// Project directory or `.sau` file. Defaults to the current directory.
    #[arg(value_name = "TARGET", value_parser = path_arg)]
    pub target: Option<PathBuf>,

    /// Arguments written straight after the target, without a `--`.
    ///
    /// The first positional is the target and every one after it is script
    /// argv, which is unambiguous and is how `python`, `node` and `lua` all
    /// read their own command lines. Requiring the separator for the
    /// ordinary case — `saule run tool.sau input.md` — turned the common
    /// invocation into an error and the rare one into the only spelling.
    ///
    /// An argument that starts with `-` still needs `--` before it, since
    /// otherwise it is this command's own flag.
    #[arg(value_name = "ARGS")]
    pub trailing: Vec<String>,

    /// Arguments forwarded to the script's `Os.args()`, after a `--`.
    ///
    /// Still the way to give argv to the *project* in the current directory
    /// (`saule run -- input.bf`, where the filename must not be read as the
    /// target), and the only way to pass one that begins with `-`.
    #[arg(last = true, allow_hyphen_values = true, value_name = "ARGS")]
    pub args: Vec<String>,

    /// Execute with the bytecode VM. This is the default; the flag remains
    /// so a script can state the engine it means rather than rely on it.
    ///
    /// Passing it also restores the `note:` line the fallback prints, which
    /// is suppressed when the VM is merely the default.
    #[arg(long, conflicts_with = "interp")]
    pub vm: bool,

    /// Execute with the tree-walking interpreter instead of the bytecode VM.
    ///
    /// The escape hatch for the Phase 4 default flip: the two engines are
    /// held to identical observable behaviour by the differential harness,
    /// so this should never be needed — and if it ever is, that is a bug
    /// worth reporting, with the program that needs it. Also selected by
    /// `SAULE_ENGINE=interp`.
    #[arg(long, conflicts_with = "vm")]
    pub interp: bool,

    /// Count what the bytecode VM executes and print an opcode and
    /// opcode-pair histogram to stderr when the program finishes.
    ///
    /// The collector `VM_DESIGN.md` §16 requires before a superinstruction
    /// is added: it reports which opcodes dominate a run and which
    /// *statically adjacent* pairs are worth fusing. Implies `--vm`, so a
    /// program the compiler cannot handle says so rather than producing an
    /// empty profile without explanation.
    ///
    /// Needs a binary built with the `profile` feature — the counting loop
    /// is not compiled otherwise, and this flag says so rather than
    /// reporting an empty run:
    ///
    ///     cargo build --release --features profile -p saule-cli
    ///
    /// Measure a release build. A debug build's costs are not the costs the
    /// optimisation is aimed at.
    #[arg(long = "profile-bytecode", conflicts_with = "interp")]
    pub profile_bytecode: bool,
}

impl RunArgs {
    /// Everything the script sees as `Os.args()`.
    ///
    /// The two spellings concatenate in the order they were written, so
    /// `saule run t.sau a -- -v` reaches the script as `["a", "-v"]`.
    pub fn script_args(&self) -> Vec<String> {
        let mut out = self.trailing.clone();
        out.extend(self.args.iter().cloned());
        out
    }
}

/// `saule init <name>`
#[derive(Debug, Args)]
#[command(about = "Scaffold a new Saule project in ./<name>")]
pub(crate) struct InitArgs {
    /// Directory to create; also the project's `name:`.
    #[arg(value_name = "NAME")]
    pub name: String,

    /// Scaffold a library — importable by other projects, with no entry point.
    #[arg(long)]
    pub lib: bool,
}
