//! `textDocument/completion` — a parse-driven completion engine.
//!
//! The cursor position is turned into a real AST node rather than guessed at
//! from the surrounding characters: the partial word under the caret is
//! replaced with a unique **sentinel identifier** and the patched source is
//! parsed. Wherever that sentinel lands in the tree *is* the context —
//!
//! * `Expr::Member { obj, name: SENTINEL }` → complete members of `obj`
//! * `Type::Named(SENTINEL)`               → complete type names
//! * `Expr::Ident(SENTINEL)`               → complete values in scope
//! * `Decl::Class { extends: SENTINEL }`   → complete base classes
//!
//! Two things fall out of this for free. A caret inside a comment or a string
//! swallows the sentinel into a trivia/literal token, so it never appears in
//! the tree and no suggestions are offered — no textual guard needed. And `..`
//! (concatenation) parses as a binary operator rather than member access, so
//! it can't be mistaken for a `.` field lookup.
//!
//! Candidates are then drawn strictly from what is *visible* at that node: a
//! scope stack built while descending to it (parameters, `local`s declared
//! earlier in the enclosing blocks, loop and catch bindings, the enclosing
//! class's own members) plus the module's imports. Nothing is offered that the
//! position cannot actually use.

mod infer;
mod items;
mod repair;
#[cfg(test)]
mod tests;
mod walk;

pub(crate) use infer::*;
pub(crate) use items::*;
pub(crate) use repair::*;
pub(crate) use walk::*;

use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind, CompletionResponse, Position, Url};

use super::{Backend, canonical};
use crate::line_index::LineIndex;

impl Backend {
    pub(super) async fn completion_at(
        &self,
        uri: &Url,
        pos: Position,
    ) -> Option<CompletionResponse> {
        let entry = self.docs.get(uri.as_str())?;
        let source = entry.source.clone();
        drop(entry);

        let line_index = LineIndex::new(&source);
        let offset = line_index.offset(&source, pos);
        let (patched, prefix) = splice_sentinel(&source, offset)?;

        // `class Foo ext…` — the only position the tree can't resolve, and
        // the only one nothing else can be meant at, so it answers alone.
        let header = header_keywords(&source, offset);
        if !header.is_empty() {
            let items = keyword_items(&header, "class header");
            return Some(CompletionResponse::Array(filter(items, &prefix)));
        }

        let module_dir = uri
            .to_file_path()
            .ok()
            .and_then(|p| canonical(&p))
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));

        // The semantic passes write thread-local registries; serialise them.
        let _guard = self.analysis_lock.lock().await;
        self.install_project_for(module_dir.as_deref()).await;

        // The sentinel only renames an identifier, so the document's
        // remembered shape still describes this text. Asked of the database
        // rather than parsed here, because the patched buffer is not the
        // document and must never be what the shape is learned from.
        let prior = uri
            .to_file_path()
            .ok()
            .and_then(|p| canonical(&p))
            .and_then(|abs| self.with_db(|db| db.prior_shape(&abs)));
        let module = parse_tolerant(&patched, prior.as_ref())?;

        // Populate the class / enum / interface registries with this module's
        // declarations *and* everything it imports, so member lookups and type
        // names resolve across files.
        let seed = match &module_dir {
            Some(d) => self.import_seed(uri, &module, d),
            None => saule_semantic::ModuleSeed::default(),
        };
        let _ = saule_semantic::analyze_with_seed(&module, seed);

        let Some(found) = Walk::run(&module) else {
            // Nothing in the tree stands for the caret. An interface body is
            // the one position where that is itself the answer.
            if Walk::in_interface_body(&module, offset - prefix.len()) {
                let items = keyword_items(INTERFACE_KEYWORDS, "interface member");
                return Some(CompletionResponse::Array(filter(items, &prefix)));
            }
            return None;
        };

        let items = match &found.ctx {
            Ctx::Member(recv) => member_items(recv, &found),
            Ctx::TypeName => type_items(),
            Ctx::BaseClass { exclude } => class_items(exclude),
            Ctx::Interfaces { exclude } => interface_items(exclude),
            Ctx::Value { stmt_start } => value_items(&found, &module, *stmt_start),
            Ctx::AfterExport => export_items(),
            Ctx::ClassMember {
                is_static,
                is_private,
            } => class_member_items(*is_static, *is_private),
            Ctx::ImportPath { quoted } => self.import_path_items(module_dir.as_deref(), *quoted),
            Ctx::ImportName { path } => import_name_items(path, module_dir.as_deref()),
        };

        Some(CompletionResponse::Array(filter(items, &prefix)))
    }

    /// Modules that can be imported from here: installed native packages plus
    /// every `.sau` file and folder module in the workspace, expressed the way
    /// the author is already spelling paths.
    fn import_path_items(
        &self,
        dir: Option<&std::path::Path>,
        quoted: bool,
    ) -> Vec<CompletionItem> {
        let sep = if quoted { "/" } else { "." };
        let mut items = Vec::new();

        // Only packages that actually require an import. Anything with
        // `auto_prelude` (the stdlib: `table`, `math`, `io`, …) is already
        // installed into every scope, so importing it is meaningless.
        for pkg in saule_interpreter::native_packages::all() {
            if pkg.auto_prelude {
                continue;
            }
            items.push(sorted(
                item(
                    pkg.name.to_string(),
                    CompletionItemKind::MODULE,
                    Some("native package".into()),
                ),
                "0",
            ));
        }
        for name in saule_interpreter::dynamic_packages::package_names() {
            let n = saule_interpreter::dynamic_packages::export_names(&name).len();
            items.push(sorted(
                item(
                    name,
                    CompletionItemKind::MODULE,
                    Some(format!("native package ({n} exported classes)")),
                ),
                "0",
            ));
        }

        // Dependencies declared in `saule.config`. These live outside the
        // workspace (`../json`) and are not under the project's own
        // `src_dirs`, so the workspace scan below never reaches them — without
        // this, a dependency you can import is a dependency you get no
        // suggestion for.
        for dep in saule_project::get()
            .map(|p| p.dependencies)
            .unwrap_or_default()
        {
            // The package itself is importable by bare name only when it
            // exposes an `init.sau` — the same rule `resolve_import_path`
            // applies, so completion never offers a path that won't resolve.
            let has_init = dep
                .src_dirs
                .iter()
                .any(|d| d.join("init.sau").is_file() || d.join("init.saule").is_file());
            if has_init {
                items.push(sorted(
                    item(
                        dep.name.clone(),
                        CompletionItemKind::MODULE,
                        Some("package".into()),
                    ),
                    "0",
                ));
            }

            // …and every module inside it, as `<dep>/<path>`. A barrel stands
            // for its folder, matching how the workspace scan below treats one.
            for src_dir in &dep.src_dirs {
                for file in saule_project::scan_sources(src_dir) {
                    let is_barrel = file.file_stem().and_then(|s| s.to_str()) == Some("init");
                    let target = if is_barrel {
                        match file.parent() {
                            Some(p) => p.to_path_buf(),
                            None => continue,
                        }
                    } else {
                        file.with_extension("")
                    };
                    let Ok(rel) = target.strip_prefix(src_dir) else {
                        continue;
                    };
                    let Some(rel) = rel.to_str() else { continue };
                    if rel.is_empty() {
                        continue;
                    }
                    let path = format!("{}/{}", dep.name, rel.replace('\\', "/"));
                    if !quoted && !path.split('/').all(is_ident_segment) {
                        continue;
                    }
                    items.push(sorted(
                        item(
                            path.replace('/', sep),
                            CompletionItemKind::MODULE,
                            Some(format!("module in `{}`", dep.name)),
                        ),
                        "1",
                    ));
                }
            }
        }

        // Workspace files, relative to this file's folder first, then to the
        // project's `src_dirs`. A folder holding `init.sau` is offered as the
        // folder itself, since that is what you import.
        let src_dirs: Vec<std::path::PathBuf> = saule_project::get()
            .map(|p| p.src_dirs.clone())
            .unwrap_or_default();

        let mut seen: Vec<String> = Vec::new();
        for entry in self.workspace_files.iter() {
            let file = entry.key().clone();
            let is_barrel = file.file_stem().and_then(|s| s.to_str()) == Some("init");
            // For a barrel the importable thing is its directory.
            let target = if is_barrel {
                match file.parent() {
                    Some(p) => p.to_path_buf(),
                    None => continue,
                }
            } else {
                file.with_extension("")
            };

            let mut bases: Vec<&std::path::Path> = Vec::new();
            if let Some(d) = dir {
                bases.push(d);
            }
            for sd in &src_dirs {
                bases.push(sd.as_path());
            }

            let rel = bases
                .iter()
                .find_map(|base| target.strip_prefix(base).ok())
                .and_then(|r| r.to_str())
                .map(|r| r.replace('\\', "/"));

            let Some(rel) = rel else { continue };
            if rel.is_empty() {
                continue;
            }
            let label = rel.replace('/', sep);
            // A bare path can only be written with identifier segments.
            if !quoted && !label.split('.').all(is_ident_segment) {
                continue;
            }
            if seen.contains(&label) {
                continue;
            }
            seen.push(label.clone());

            let detail = if is_barrel {
                "folder module".to_string()
            } else {
                "module".to_string()
            };
            items.push(sorted(
                item(label, CompletionItemKind::FILE, Some(detail)),
                "1",
            ));
        }

        items
    }
}
