//! `saule init <name>` — scaffold a new Saule project.

use std::{fs, path::PathBuf, process};

pub(crate) fn cmd_init(name: &str) {
    let root = PathBuf::from(name);
    if root.exists() {
        eprintln!("error: `{name}` already exists");
        process::exit(1);
    }
    if let Err(e) = fs::create_dir_all(root.join("src")) {
        eprintln!("error creating project directory: {e}");
        process::exit(1);
    }

    // The two `indent_*` keys are scaffolded commented-out: they show the
    // knob (and the defaults `saule fmt` and the LSP already use) without
    // committing a new project to anything.
    let config = format!(
        "name: \"{name}\"\n\
         version: \"0.1.0\"\n\
         entry: \"src/main.sau\"\n\
         src_dirs: [\"src\"]\n\
         min_saule_version: \"{}\"\n\
         \n\
         -- Formatting, shared by `saule fmt` and the editor's Reformat.\n\
         -- indent_style: \"space\"  -- or \"tab\"\n\
         -- indent_width: 2        -- columns, 1..=16\n",
        env!("CARGO_PKG_VERSION")
    );

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
        println(\"Hello, \" .. self.who)
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
