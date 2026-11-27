//! `saule build`: a program as one executable, and running one.
//!
//! **Building** compiles the program to bytecode (`saule_vm::bytecode`) and
//! appends it to a copy of this very binary, with the native packages it
//! imports and the files the project bundles ([`archive`] has the layout).
//!
//! **Running**: every start of `saule` first checks for such an archive
//! ([`run_embedded`]). A plain toolchain has none and carries on as the
//! CLI; a built app runs its program instead, with every argument passed to
//! it as `Os.args()`. Its bundled files and native packages are unpacked
//! once into a cache directory ([`cache`]), which then serves as the
//! program's `Project.root` and as the Saule home its packages load from.
//!
//! **Shared** (`saule build --shared`): the output is the archive alone,
//! behind a first line that names the saule to run it,
//!
//! ```text
//! #!/bin/saule run
//! ```
//!
//! so that every program on a machine runs on one runtime instead of each
//! carrying a copy. `saule run FILE` is what that line asks for
//! ([`run_given`]): a file that ends in an archive is run as the program in
//! it. Its native packages are not in the file either: they are loaded from
//! the home of the saule that runs it.

mod archive;
mod cache;

use std::path::{Path, PathBuf};
use std::process;

use miette::{NamedSource, Report};

use crate::cli::BuildArgs;
use archive::Entry;

/// What a built program needs to know about itself before it runs.
struct Manifest {
    name: String,
    version: String,
    /// Built from a project rather than a single file: it must declare
    /// `Main.main`, and its `Project` fields are filled in.
    require_main: bool,
    /// Built with `--shared`: its native packages are the runtime's own,
    /// in the Saule home, and not unpacked beside its files.
    shared: bool,
}

impl Manifest {
    fn write(&self) -> Vec<u8> {
        format!(
            "name={}\nversion={}\nrequire_main={}\nshared={}\n",
            self.name, self.version, self.require_main, self.shared
        )
        .into_bytes()
    }

    fn read(bytes: &[u8]) -> Manifest {
        let text = String::from_utf8_lossy(bytes);
        let mut m = Manifest {
            name: "app".into(),
            version: String::new(),
            require_main: true,
            shared: false,
        };
        for line in text.lines() {
            match line.split_once('=') {
                Some(("name", v)) => m.name = v.into(),
                Some(("version", v)) => m.version = v.into(),
                Some(("require_main", v)) => m.require_main = v == "true",
                Some(("shared", v)) => m.shared = v == "true",
                _ => {}
            }
        }
        m
    }
}

// ─── Building ───────────────────────────────────────────────────────────────

pub(crate) fn cmd_build(args: BuildArgs) {
    let target = args.target.clone().unwrap_or_else(|| PathBuf::from("."));
    let exe = std::env::current_exe().unwrap_or_else(|e| {
        eprintln!("error: can't find the saule binary to build with: {e}");
        process::exit(1);
    });
    // Before anything is compiled: a path that can't be written in the
    // output's first line is a mistake in the command.
    let runtime = args.shared.as_deref().map(|given| runtime_for(given, &exe));
    let shared = runtime.is_some();

    let (entry, manifest, root, bundle) = if target.is_dir() {
        let project = crate::project::configure_project(&target, true);
        let entry = project.entry.expect("configure_project requires an entry");
        let info = saule_project::get().expect("configure_project installs the project");
        let config = saule_project::Config::read_in(&info.root).unwrap_or_default();
        let manifest = Manifest {
            name: info.name.clone(),
            version: info.version.clone(),
            require_main: true,
            shared,
        };
        (entry, manifest, info.root, config.bundle)
    } else if target.is_file() {
        let name = target
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "app".into());
        let root = target
            .canonicalize()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        let manifest = Manifest {
            name,
            version: String::new(),
            require_main: false,
            shared,
        };
        // A lone file bundles nothing unless it sits in a project's folder
        // structure the user asked for — and a file has no config to ask in.
        (target.clone(), manifest, root, Some(Vec::new()))
    } else {
        eprintln!("error: `{}` does not exist", target.display());
        process::exit(1);
    };

    // Origins have to be recorded while compiling: they are how the folded
    // standard library constants are written out.
    saule_vm::bytecode::origin::enable();
    let program = compile(&entry);

    let source = if args.keep_source {
        saule_vm::bytecode::Source::Full
    } else {
        saule_vm::bytecode::Source::Lines
    };
    let bytecode =
        saule_vm::bytecode::encode(&program, source, &relabel(&root)).unwrap_or_else(|e| {
            eprintln!("error: {e}");
            process::exit(1);
        });

    let mut entries = vec![
        Entry {
            path: archive::MANIFEST.into(),
            data: manifest.write(),
        },
        Entry {
            path: archive::PROGRAM.into(),
            data: bytecode,
        },
    ];
    let native = native_packages(&program, &mut entries, !shared);
    let files = bundled_files(&root, bundle, &mut entries);

    let out = args
        .output
        .clone()
        .unwrap_or_else(|| root.join("build").join(&manifest.name));
    let packed = archive::pack(&entries);
    let runner = match &runtime {
        Some(runtime) => {
            write_output(&out, first_line(runtime).into_bytes(), &packed);
            None
        }
        None => Some(write_executable(&exe, &out, &packed)),
    };

    let program_size = entries
        .iter()
        .find(|e| e.path == archive::PROGRAM)
        .map_or(0, |e| e.data.len());
    let total = std::fs::metadata(&out).map_or(0, |m| m.len());
    println!("built {} ({})", out.display(), size(total));
    match (&runtime, runner) {
        (Some(runtime), _) => println!("  runtime          {} (shared)", runtime.display()),
        (None, runner) => println!("  runtime          {}", size(runner.unwrap_or(0))),
    }
    println!(
        "  program          {} (bytecode)",
        size(program_size as u64)
    );
    if native.0 > 0 && shared {
        println!("  native packages  {} (the runtime's own)", native.0);
    } else if native.0 > 0 {
        println!("  native packages  {} ({})", size(native.1), native.0);
    }
    if files.0 > 0 {
        println!("  files            {} ({})", size(files.1), files.0);
    }
}

/// Check and compile, reporting errors the way `saule run` does.
fn compile(entry: &Path) -> saule_vm::program::Program {
    let source = std::fs::read_to_string(entry).unwrap_or_else(|e| {
        eprintln!("error reading '{}': {e}", entry.display());
        process::exit(1);
    });
    let name = entry
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| entry.display().to_string());
    match crate::run::compile_entry(&name, source, entry) {
        Ok((_, program)) => program,
        Err(report) => {
            eprintln!("{report:?}");
            process::exit(1);
        }
    }
}

/// How a module is named inside the executable, for its errors and stack
/// traces: relative to the project, or `<package>/<path>` for a Saule
/// package — never an absolute path on the machine that built it.
fn relabel(root: &Path) -> impl Fn(&str) -> String {
    let canonical = |p: PathBuf| p.canonicalize().unwrap_or(p);
    let root = canonical(root.to_path_buf());
    let packages = canonical(saule_project::home::source_packages_dir());
    let slash = |parts: &[std::path::Component]| {
        parts
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/")
    };
    move |label: &str| {
        let path = Path::new(label);
        if !path.is_absolute() {
            return label.to_string();
        }
        if let Ok(rest) = path.strip_prefix(&packages) {
            // <host>/<owner>/<repo>/<ref>/<file…> → <repo>/<file…>
            let parts: Vec<_> = rest.components().collect();
            if parts.len() > 4 {
                return format!("{}/{}", slash(&parts[2..3]), slash(&parts[4..]));
            }
        }
        if let Ok(rest) = path.strip_prefix(&root) {
            return slash(&rest.components().collect::<Vec<_>>());
        }
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| label.to_string())
    }
}

/// Every dynamic native package the program imports, as archive entries
/// when `packed`: a shared program leaves them to the runtime that runs
/// it, and need not have them where it is built. Returns how many, and
/// their size.
fn native_packages(
    program: &saule_vm::program::Program,
    entries: &mut Vec<Entry>,
    packed: bool,
) -> (usize, u64) {
    let mut names: Vec<&str> = program
        .modules
        .iter()
        .flat_map(|c| c.dynamic_imports.iter().map(|(name, _)| name.as_str()))
        .collect();
    names.sort_unstable();
    names.dedup();

    let mut bytes = 0;
    for name in names.iter().filter(|_| packed) {
        let Some(path) = saule_runtime::dynamic_packages::package_path(name) else {
            eprintln!("error: native package `{name}` is imported but not installed");
            process::exit(1);
        };
        let data = std::fs::read(&path).unwrap_or_else(|e| {
            eprintln!("error reading native package `{}`: {e}", path.display());
            process::exit(1);
        });
        let file = path.file_name().expect("a library file").to_string_lossy();
        bytes += data.len() as u64;
        entries.push(Entry {
            path: format!("{}{file}", archive::NATIVE),
            data,
        });
    }
    (names.len(), bytes)
}

/// The files `bundle:` lists, as archive entries under their path relative
/// to the project root. Unset means `assets`, if there is one.
fn bundled_files(
    root: &Path,
    bundle: Option<Vec<String>>,
    entries: &mut Vec<Entry>,
) -> (usize, u64) {
    let explicit = bundle.is_some();
    let listed = bundle.unwrap_or_else(|| vec!["assets".into()]);

    let mut found = Vec::new();
    for item in &listed {
        let path = root.join(item);
        if !path.exists() {
            if explicit {
                eprintln!("error: `{item}` is listed under `bundle:` but does not exist");
                process::exit(1);
            }
            continue;
        }
        collect(&path, &mut found);
    }
    found.sort();
    found.dedup();

    let mut bytes = 0;
    for path in &found {
        let rel = path.strip_prefix(root).unwrap_or(path);
        let rel: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let data = std::fs::read(path).unwrap_or_else(|e| {
            eprintln!("error reading '{}': {e}", path.display());
            process::exit(1);
        });
        bytes += data.len() as u64;
        entries.push(Entry {
            path: format!("{}{}", archive::FILES, rel.join("/")),
            data,
        });
    }
    (found.len(), bytes)
}

fn collect(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        let Ok(read) = std::fs::read_dir(path) else {
            return;
        };
        for entry in read.flatten() {
            collect(&entry.path(), out);
        }
    } else if path.is_file() {
        out.push(path.to_path_buf());
    }
}

/// Write the runner part of `exe` plus `archive` to `out`, executable.
/// Returns the runner's size.
fn write_executable(exe: &Path, out: &Path, archive: &[u8]) -> u64 {
    let fail = |what: &str, e: std::io::Error| -> ! {
        eprintln!("error: {what} '{}': {e}", out.display());
        process::exit(1);
    };
    let runner_len = archive::runner_len(exe).unwrap_or_else(|e| fail("reading saule for", e));
    let mut bytes = std::fs::read(exe).unwrap_or_else(|e| fail("reading saule for", e));
    bytes.truncate(runner_len as usize);
    write_output(out, bytes, archive);
    runner_len
}

/// The saule a shared program names in its first line: `given`, or this
/// one when no path was. It is written there as it is, so it has to be a
/// whole path the operating system can read off that line.
fn runtime_for(given: &Path, exe: &Path) -> PathBuf {
    let refuse = |why: &str| -> ! {
        eprintln!("error: --shared: {why}");
        process::exit(1);
    };
    let runtime = if given.as_os_str().is_empty() {
        // This binary with a program packed into it is not a saule that
        // runs other programs.
        let whole = std::fs::metadata(exe).map_or(0, |m| m.len());
        if archive::runner_len(exe).ok() != Some(whole) {
            refuse("this saule is a built program; give the path of a saule to share");
        }
        exe.canonicalize().unwrap_or_else(|_| exe.to_path_buf())
    } else {
        given.to_path_buf()
    };
    let Some(text) = runtime.to_str() else {
        refuse("the path of the saule is not text");
    };
    if !runtime.is_absolute() {
        refuse(&format!(
            "`{text}` is not a whole path (it is where the saule is on the machine the program runs on)"
        ));
    }
    if text.chars().any(|c| c.is_whitespace()) {
        refuse(&format!(
            "`{text}` has a space in it, which the first line of a program cannot carry"
        ));
    }
    // The shortest any system reads of that line.
    if first_line(&runtime).len() > 127 {
        refuse(&format!("`{text}` is too long for the first line of a program"));
    }
    runtime
}

/// What a shared program starts with: the operating system runs the saule
/// named there, with `run`, this file and the program's own arguments.
fn first_line(runtime: &Path) -> String {
    format!("#!{} run\n", runtime.display())
}

/// Write `bytes` and then `archive` to `out`, executable.
fn write_output(out: &Path, mut bytes: Vec<u8>, archive: &[u8]) {
    let fail = |what: &str, e: std::io::Error| -> ! {
        eprintln!("error: {what} '{}': {e}", out.display());
        process::exit(1);
    };
    bytes.extend_from_slice(archive);

    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).unwrap_or_else(|e| fail("creating the folder for", e));
    }
    // Written beside and renamed, so a running copy of the old build is
    // never overwritten in place.
    let tmp = out.with_extension("tmp-build");
    std::fs::write(&tmp, &bytes).unwrap_or_else(|e| fail("writing", e));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
    }
    std::fs::rename(&tmp, out).unwrap_or_else(|e| fail("writing", e));
}

fn size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.0} KB", b as f64 / (1u64 << 10) as f64),
        b => format!("{b} B"),
    }
}

// ─── Running ────────────────────────────────────────────────────────────────

/// Run the program packed into this executable, if there is one, and
/// return its exit code. `None` means this is the plain toolchain.
pub(crate) fn run_embedded() -> Option<i32> {
    let exe = std::env::current_exe().ok()?;
    let embedded = match archive::read(&exe) {
        Ok(Some(e)) => e,
        Ok(None) => return None,
        Err(e) => {
            eprintln!("error: {e}");
            return Some(1);
        }
    };

    Some(run(&exe, &embedded, std::env::args().skip(1).collect()))
}

/// `saule run FILE [ARGS]` where FILE is a program `saule build --shared`
/// made (or any built program): run it on this saule, and return its exit
/// code. `None` when the command line is anything else, which is then
/// read as usual. What follows the file is the program's, whatever it
/// looks like, and never an option of saule's.
pub(crate) fn run_given() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next()? != "run" {
        return None;
    }
    let file = PathBuf::from(args.next()?);
    if !file.is_file() {
        return None;
    }
    let embedded = match archive::read(&file) {
        Ok(Some(e)) => e,
        Ok(None) => return None,
        Err(e) => {
            eprintln!("error: {}: {e}", file.display());
            return Some(1);
        }
    };
    Some(run(&file, &embedded, std::env::args().skip(3).collect()))
}

/// Run the program in `embedded`, the archive at the end of `file`, with
/// `args` as its `Os.args()`. Its exit code.
fn run(file: &Path, embedded: &archive::Embedded, args: Vec<String>) -> i32 {
    let manifest = Manifest::read(embedded.get(archive::MANIFEST).unwrap_or_default());
    let Some(bytecode) = embedded.get(archive::PROGRAM) else {
        eprintln!("error: {} has no program in it", file.display());
        return 1;
    };

    // Bundled files and packages live on disk, where the program and the
    // loader expect them; with neither, the file's folder is the root. A
    // shared program's packages are the runtime's, in the home it has.
    let root = match cache::unpack(&manifest.name, embedded) {
        Ok(Some(dir)) => {
            if !manifest.shared {
                saule_project::home::set_override(dir.clone());
            }
            dir.join(archive::FILES.trim_end_matches('/'))
        }
        Ok(None) => file
            .parent()
            .filter(|dir| !dir.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(".")),
        Err(e) => {
            eprintln!("error: can't unpack {}: {e}", manifest.name);
            return 1;
        }
    };

    // A single file has no project under `saule run` — every `Project`
    // field is empty — and stays that way built.
    if manifest.require_main {
        saule_project::set(saule_project::ProjectInfo {
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            root,
            src_dirs: Vec::new(),
            dependencies: Vec::new(),
        });
    }
    saule_runtime::stdlib::os::set_script_args(args);
    saule_runtime::init();

    let program = match saule_vm::bytecode::decode(bytecode) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };

    let source: NamedSource<String> = (*program.entry_chunk().source).clone();
    // `SAULE_PROFILE=1` (or a file): a built app has no command line of
    // its own to ask for a profile with.
    saule_vm::sampling::start_from_env();
    let outcome = saule_vm::run_program(program);
    saule_vm::sampling::finish();
    match outcome {
        Ok(true) => 0,
        Ok(false) if !manifest.require_main => 0,
        Ok(false) => {
            eprintln!("error: the program has no `class Main` with a `static fn main()`");
            1
        }
        Err(e) => {
            eprintln!("{:?}", Report::new(e).with_source_code(source));
            crate::run::print_stack_trace(&saule_vm::take_stack_trace());
            1
        }
    }
}
