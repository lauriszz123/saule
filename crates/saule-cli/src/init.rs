//! `saule init <name>` — scaffold a new Saule project.
//!
//! Two shapes: an **app**, which has an `entry:` and is run with `saule run`,
//! and a **library** (`--lib`), which has no entry point and exists to be
//! imported. A library exposes itself through `src/init.sau` — the same
//! folder-module rule imports use everywhere else.

use std::{fs, path::PathBuf, process};

pub(crate) fn cmd_init(name: &str, lib: bool) {
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
    // A library declares `kind:` and omits `entry:`; an app is the default
    // and says neither.
    let shape = if lib {
        "kind: \"library\"\n".to_string()
    } else {
        "entry: \"src/main.sau\"\n".to_string()
    };
    let config = format!(
        "name: \"{name}\"\n\
         version: \"0.1.0\"\n\
         {shape}\
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

    // `init.sau` is a library's public surface: whatever it exports is what
    // `import ... from "<name>"` gets.
    let init_sau = format!(
        "--[[\n\
         Public surface of the `{name}` library.\n\
         \n\
         Whatever this file exports is what `import ... from \"{name}\"` sees.\n\
         ]]\n\
         \n\
         export class {Name}\n\
         \x20   static fn greet(who: string) -> string\n\
         \x20       return \"Hello, \" .. who\n\
         \x20   end\n\
         end\n",
        name = name,
        Name = capitalise(name),
    );

    let gitignore = "*.log\n";

    let readme = if lib {
        format!(
            "# {name}\n\n\
             A Saule library. Add it to another project's `saule.config`:\n\n\
             ```\n\
             dependencies: [\"../{name}\"]\n\
             ```\n\n\
             then import it:\n\n\
             ```saule\n\
             import {Name} from \"{name}\"\n\
             ```\n",
            name = name,
            Name = capitalise(name),
        )
    } else {
        format!("# {name}\n\nA Saule project. Run with:\n\n```sh\nsaule run\n```\n")
    };

    let write = |relpath: &str, contents: &str| -> Result<(), std::io::Error> {
        fs::write(root.join(relpath), contents)
    };

    let entry = if lib { "src/init.sau" } else { "src/main.sau" };
    let body = if lib { init_sau.as_str() } else { main_sau };

    if let Err(e) = write("saule.config", &config)
        .and_then(|_| write(entry, body))
        .and_then(|_| write(".gitignore", gitignore))
        .and_then(|_| write("README.md", &readme))
    {
        eprintln!("error writing project files: {e}");
        process::exit(1);
    }

    if lib {
        println!("Created library `{name}`");
        println!("  edit {name}/src/init.sau — its exports are the package's API");
        println!("  depend on it with  dependencies: [\"../{name}\"]");
    } else {
        println!("Created project `{name}`");
        println!("  cd {name}");
        println!("  saule run");
    }
}

/// Upper-case the first character, so `json` scaffolds a `Json` class.
fn capitalise(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capitalise_uppercases_only_the_first_character() {
        assert_eq!(capitalise("json"), "Json");
        assert_eq!(capitalise("myLib"), "MyLib");
        assert_eq!(capitalise(""), "");
    }
}
