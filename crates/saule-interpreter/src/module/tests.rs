use super::*;

/// Build a throwaway package on disk: `<root>/<name>/src/<file>`.
fn make_pkg(root: &Path, name: &str, file: &str) -> crate::project::Dependency {
    let src = root.join(name).join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join(file), "-- test module\n").unwrap();
    crate::project::Dependency {
        name: name.to_string(),
        root: root.join(name),
        src_dirs: vec![src],
    }
}

fn with_deps(root: &Path, deps: Vec<crate::project::Dependency>) {
    crate::project::set(crate::project::ProjectInfo {
        name: "app".into(),
        version: "0.0.0".into(),
        root: root.to_path_buf(),
        src_dirs: vec![root.join("src")],
        dependencies: deps,
    });
}

fn tmp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("saule-mod-{tag}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    // Resolution canonicalises, and on macOS `/var` is a symlink to
    // `/private/var` — compare against the resolved form.
    dir.canonicalize().unwrap_or(dir)
}

#[test]
fn bare_dependency_name_resolves_to_its_init_module() {
    // `import X from "json"` names the package itself, which resolves to
    // the one thing a package may expose: `src/init.sau`.
    let root = tmp("init");
    let dep = make_pkg(&root, "json", "init.sau");
    let expected = dep.src_dirs[0].join("init.sau");
    with_deps(&root, vec![dep]);

    assert_eq!(resolve_import_path(&root, "json"), Some(expected));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_package_without_init_is_not_importable_by_name() {
    // Only `init.sau` marks a package's entry point — a module that
    // merely shares the package's name does not stand in for one.
    let root = tmp("noinit");
    with_deps(&root, vec![make_pkg(&root, "json", "json.sau")]);

    assert_eq!(resolve_import_path(&root, "json"), None);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn modules_inside_a_dependency_are_still_reachable() {
    // The package's own files stay importable by path, which is how an
    // `init.sau` barrel re-exports them.
    let root = tmp("qualified");
    let dep = make_pkg(&root, "json", "init.sau");
    let src = dep.src_dirs[0].clone();
    std::fs::write(src.join("lexer.sau"), "-- lexer\n").unwrap();
    with_deps(&root, vec![dep]);

    assert_eq!(
        resolve_import_path(&root, "json/lexer"),
        Some(src.join("lexer.sau"))
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn an_unknown_package_name_resolves_to_nothing() {
    let root = tmp("unknown");
    with_deps(&root, vec![make_pkg(&root, "json", "json.sau")]);
    assert_eq!(resolve_import_path(&root, "nope"), None);
    std::fs::remove_dir_all(&root).ok();
}

/// Parse `src` as a module, for the overlay tests below.
fn parse_module(src: &str) -> saule_ast::Module {
    let tokens = saule_lexer::Lexer::new(src).tokenize().unwrap();
    saule_parser::parse(tokens).unwrap()
}

#[test]
fn the_import_seed_reads_imports_from_disk_by_default() {
    // Baseline for the overlay test below: with no overlay, the seed sees
    // exactly what is on disk.
    let root = tmp("seed-disk");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("lib.sau"),
        "export class Widget\n\tfn spin() end\nend\n",
    )
    .unwrap();

    let importer = parse_module("import Widget from \"lib\"\n");
    let seed = collect_import_seed(&importer, &src);

    assert!(
        seed.classes.contains_key("Widget"),
        "on-disk class should be seeded"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn an_overlay_supersedes_the_on_disk_copy_of_an_import() {
    // The language-server case: the editor holds an unsaved buffer whose
    // contents differ from the file. The importer's analysis has to see the
    // buffer, not the stale file — otherwise renaming a class in one file
    // leaves every importer reporting against the old name until you save.
    let root = tmp("seed-overlay");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    let lib = src.join("lib.sau");
    std::fs::write(&lib, "export class OldName\n\tfn spin() end\nend\n").unwrap();

    let importer = parse_module("import NewName from \"lib\"\n");
    let buffer = "export class NewName\n\tfn spin() end\nend\n".to_string();
    let lib_canonical = lib.canonicalize().unwrap_or_else(|_| lib.clone());

    let seed = collect_import_seed_with(&importer, &src, &|p: &Path| {
        (p == lib_canonical || p == lib).then(|| buffer.clone())
    });

    assert!(
        seed.classes.contains_key("NewName"),
        "the unsaved buffer's class should be seeded"
    );
    assert!(
        !seed.classes.contains_key("OldName"),
        "the stale on-disk class must not leak through"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn an_overlay_that_declines_falls_back_to_disk() {
    // Returning `None` means "I have nothing for this path" — the walk must
    // then read the file as usual. Files the editor has not opened take this
    // path on every request.
    let root = tmp("seed-passthrough");
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("lib.sau"),
        "export class Widget\n\tfn spin() end\nend\n",
    )
    .unwrap();

    let importer = parse_module("import Widget from \"lib\"\n");
    let seed = collect_import_seed_with(&importer, &src, &|_| None);

    assert!(
        seed.classes.contains_key("Widget"),
        "declining the overlay should fall back to the file"
    );
    std::fs::remove_dir_all(&root).ok();
}
