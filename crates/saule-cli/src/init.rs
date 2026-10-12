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
        // The short form deliberately: scaffolding `26.8-dev+1a2b3c4` into a
        // new project would make the config unusable on the actual 26.8.
        saule_version::VERSION
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

    let init_sau = library_source(name);

    let gitignore = "*.log\n";

    let readme = if lib {
        library_readme(name)
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

/// `src/init.sau` for a library — its public surface: whatever this file
/// exports is what `import ... from "<name>"` gets.
fn library_source(name: &str) -> String {
    format!(
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
        Name = class_identifier(name),
    )
}

/// `README.md` for a library: how to depend on it, and how to import it.
fn library_readme(name: &str) -> String {
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
        Name = class_identifier(name),
    )
}

/// Derive a Saule class identifier from a project name.
///
/// A project is usually named for its directory — `saule-widget`,
/// `my_cool.lib`, `json` — and most of those are not identifiers: Saule's
/// are `[A-Za-z_][A-Za-z0-9_]*`. So split the name on everything that
/// cannot appear in one, upper-case each piece's first character, and
/// join. `saule-widget` scaffolds a `SauleWidget`, not a `Saule-widget`
/// that fails to parse.
///
/// Inner capitals survive, so `myLib` stays `MyLib` rather than flattening
/// to `Mylib`. `_` is a legal identifier character but is treated as a
/// separator anyway, because `MyCool` reads better than `My_cool`.
///
/// Two names cannot produce a legal identifier on their own and get a
/// `Lib` prefix instead: one that would start with a digit (`2fast` →
/// `Lib2fast`), and one with no usable characters at all (`--` → `Lib`).
/// The alternative is scaffolding a file that does not compile, which is
/// the bug this exists to prevent.
fn class_identifier(name: &str) -> String {
    let mut out = String::new();

    for segment in name.split(|c: char| !c.is_ascii_alphanumeric()) {
        let mut chars = segment.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }

    if out.is_empty() {
        return "Lib".to_string();
    }

    if out.starts_with(|c: char| c.is_ascii_digit()) {
        return format!("Lib{out}");
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_identifier_upper_cases_a_plain_name() {
        assert_eq!(class_identifier("json"), "Json");
        assert_eq!(class_identifier("myLib"), "MyLib");
    }

    #[test]
    fn class_identifier_joins_the_segments_of_a_punctuated_name() {
        assert_eq!(class_identifier("saule-widget"), "SauleWidget");
        assert_eq!(class_identifier("my_cool.lib"), "MyCoolLib");
        assert_eq!(class_identifier("a-b-c"), "ABC");
        assert_eq!(class_identifier("two words"), "TwoWords");
    }

    #[test]
    fn class_identifier_falls_back_when_the_name_cannot_start_one() {
        // A leading digit and an empty result are the only two cases that
        // cannot be fixed by capitalising, so both take the `Lib` prefix.
        assert_eq!(class_identifier("2fast"), "Lib2fast");
        assert_eq!(class_identifier("2-fast-2-furious"), "Lib2Fast2Furious");
        assert_eq!(class_identifier("--"), "Lib");
        assert_eq!(class_identifier(""), "Lib");
        assert_eq!(class_identifier("日本語"), "Lib");
    }

    /// The real contract: whatever the project is called, the file we write
    /// has to parse. Before `class_identifier`, `saule init saule-widget`
    /// emitted `export class Saule-widget` and a brand-new project failed
    /// to compile until the user renamed the class by hand.
    #[test]
    fn scaffolded_library_source_parses_for_awkward_names() {
        let names = [
            "json",
            "saule-widget",
            "my_cool.lib",
            "myLib",
            "2fast",
            "two words",
            "--",
            "",
        ];

        for name in names {
            let source = library_source(name);

            let tokens = saule_lexer::Lexer::new(&source)
                .tokenize()
                .unwrap_or_else(|e| panic!("`{name}` scaffolded source does not lex: {e:?}"));

            saule_parser::parse(tokens)
                .unwrap_or_else(|e| panic!("`{name}` scaffolded source does not parse: {e:?}"));
        }
    }
}
