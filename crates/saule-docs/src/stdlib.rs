//! Documentation for the **standard library**, parsed out of `DOCS.md`.
//!
//! The rest of this crate recovers `---` doc comments from Saule source. The
//! standard library has no Saule source to recover them from — `String.sub` is
//! a Rust function behind a [`NativeSig`] that carries types and nothing else:
//! no parameter names, no prose. So hovering a stdlib call used to show
//! `(string, integer, integer?) -> string`, which says what it takes and not
//! what it is.
//!
//! `DOCS.md` already holds every signature *with parameter names*, a
//! description, and a runnable example that CI checks. That makes it the one
//! place the library is written down, so this parses it rather than restating
//! it. The editor then cannot describe a function the documentation does not,
//! nor spell a signature the compiler would reject, and adding a function to
//! the library documents it in the editor with no second edit.
//!
//! The playground does the same thing in JavaScript
//! (`www/scripts/gen-stdlib-index.mjs`) for its own completion popup. The
//! parsing rules here deliberately match it, including the receiver mapping
//! and the anchor spelling, so the two surfaces describe the library
//! identically.
//!
//! ## What is parsed
//!
//! ```text
//! ## `String`                 → a module, its prose the module's summary
//! ### Searching               → a group, recorded as an entry's category
//! #### `String.sub(s: string, from: integer, to: integer?) -> string`
//!                             → an entry: signature, parameter names, prose
//! ```
//!
//! A `####` heading that is not a backticked signature (`#### Operators`) is
//! prose structure rather than an API entry and is skipped.
//!
//! [`NativeSig`]: https://docs.rs/saule-typeck

use std::collections::HashMap;
use std::sync::OnceLock;

/// `DOCS.md`, compiled in.
///
/// Read at build time rather than found at run time: `saule-lsp` is installed
/// as a bare binary in `~/.saule/bin` with no data directory beside it, so a
/// path lookup would work in a checkout and fail for every real user.
const DOCS: &str = include_str!("../../../DOCS.md");

/// `LANGUAGE.md`, compiled in for its anchors alone.
///
/// `DOCS.md`'s prose links into the language guide — `[Casting](./LANGUAGE.md#casting)`
/// — and a hover card has to turn that into a URL or render a dead link. Only
/// the headings are needed, so this is read for its anchor map and the prose
/// is discarded.
const LANGUAGE: &str = include_str!("../../../LANGUAGE.md");

/// Where the published reference lives, for the "full reference" link on a
/// hover card.
const DOCS_BASE: &str = "https://lauriszz123.github.io/saule";

/// One documented standard-library function, method or constant.
#[derive(Debug, Clone)]
pub struct StdlibEntry {
    /// `String.sub`, `println`, `File.read`.
    pub qname: String,
    /// `String` for `String.sub`; `None` for a prelude function.
    pub owner: Option<String>,
    /// `sub`.
    pub name: String,
    /// The signature exactly as documented, parameter names included:
    /// `String.sub(s: string, from: integer, to: integer?) -> string`.
    pub signature: String,
    /// Just the **positional** parameter names, in order:
    /// `["s", "from", "to"]`.
    ///
    /// A variadic tail is deliberately absent. `...any` declares no name for
    /// the consumer to use, and inventing one from the type would put a slot
    /// here that the signature does not have — a caller of
    /// `Math.max<N>(...N) -> N` would be told its first argument is called
    /// `N`. Naming the variadic slot is left to whoever renders it.
    pub params: Vec<String>,
    /// The first paragraph of prose — one sentence or three, enough for a
    /// completion popup.
    pub summary: String,
    /// The whole entry body as Markdown: prose, caveats and the example.
    pub body: String,
    /// The `###` group the entry sits under, such as `Searching`.
    pub group: Option<String>,
    /// URL of this entry in the published reference.
    pub url: String,
    /// True when the entry is an instance method documented on a sample
    /// receiver (`f.read`), rather than a static (`String.sub`).
    pub is_method: bool,
    /// What kind of thing this is. A constant has no parameters and its
    /// `signature` is `Name: type` rather than a call.
    pub kind: EntryKind,
}

/// Whether an entry is something you call or something you read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// A function or method, documented by a `####` signature heading.
    Function,
    /// A constant or field, documented as a row in a module's table —
    /// `Math.pi`, `Saule.version`, `Os.sep`, `Project.root`. These have no
    /// `####` heading of their own because there is nothing to say about them
    /// beyond a type and a sentence, which is exactly what the table holds.
    Constant,
}

/// One documented standard-library module — the `##` sections.
#[derive(Debug, Clone)]
pub struct StdlibModule {
    /// `String`. `Io / File` contributes both `Io` and `File`.
    pub name: String,
    /// The first paragraph under the heading.
    pub summary: String,
    /// All the prose under the heading, up to the first `###` or `####`.
    pub body: String,
    /// URL of the module's page in the published reference.
    pub url: String,
}

/// The parsed index.
#[derive(Debug, Default)]
pub struct StdlibDocs {
    entries: HashMap<String, StdlibEntry>,
    modules: HashMap<String, StdlibModule>,
    /// Every heading anchor in `DOCS.md` and `LANGUAGE.md`, mapped to the
    /// published URL it resolves to. Built so [`Self::absolute_links`] can
    /// turn the sources' in-page links into ones a hover card can follow.
    anchors: HashMap<String, String>,
}

impl StdlibDocs {
    /// Look up one entry by qualified name: `String.sub`, `println`.
    pub fn entry(&self, qname: &str) -> Option<&StdlibEntry> {
        self.entries.get(qname)
    }

    /// Look up a module by name: `String`, `Io`, `File`.
    pub fn module(&self, name: &str) -> Option<&StdlibModule> {
        self.modules.get(name)
    }

    /// Every entry belonging to `owner`, sorted by name.
    pub fn members_of(&self, owner: &str) -> Vec<&StdlibEntry> {
        let mut out: Vec<&StdlibEntry> = self
            .entries
            .values()
            .filter(|e| e.owner.as_deref() == Some(owner))
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Every entry, sorted by qualified name. For tests and for tooling that
    /// wants to audit coverage.
    pub fn all(&self) -> Vec<&StdlibEntry> {
        let mut out: Vec<&StdlibEntry> = self.entries.values().collect();
        out.sort_by(|a, b| a.qname.cmp(&b.qname));
        out
    }

    /// Rewrite a documentation body's in-page links into absolute URLs.
    ///
    /// The sources are written to be read as whole files, so they link within
    /// themselves: `](#string-sub)` and `](./LANGUAGE.md#casting)`. Rendered
    /// into a hover card those go nowhere — the editor has no page for the
    /// fragment to be relative to. This resolves each against the anchor map
    /// and leaves anything it cannot resolve exactly as written, so an
    /// unknown target degrades to inert text rather than a wrong destination.
    pub fn absolute_links(&self, md: &str) -> String {
        let mut out = String::with_capacity(md.len());
        let mut rest = md;
        while let Some(at) = rest.find("](") {
            let (before, tail) = rest.split_at(at);
            out.push_str(before);
            // The target runs to the closing paren.
            let Some(close) = tail.find(')') else {
                out.push_str(tail);
                return out;
            };
            let target = &tail[2..close];
            // `#anchor`, or `./DOCS.md#anchor` / `./LANGUAGE.md#anchor`.
            let anchor = target
                .rsplit_once('#')
                .map(|(_, a)| a)
                .filter(|_| target.starts_with('#') || target.contains(".md#"));
            match anchor.and_then(|a| self.anchors.get(a)) {
                Some(url) => {
                    out.push_str("](");
                    out.push_str(url);
                    out.push(')');
                }
                None => out.push_str(&tail[..=close]),
            }
            rest = &tail[close + 1..];
        }
        out.push_str(rest);
        out
    }

    /// The published URL for one heading anchor, if it is known.
    pub fn anchor_url(&self, anchor: &str) -> Option<&str> {
        self.anchors.get(anchor).map(String::as_str)
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    pub fn module_count(&self) -> usize {
        self.modules.len()
    }
}

/// The index, parsed once on first use.
pub fn stdlib() -> &'static StdlibDocs {
    static INDEX: OnceLock<StdlibDocs> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut docs = parse(DOCS);
        docs.anchors = collect_anchors();
        docs
    })
}

/// Map every heading anchor in both sources to the page it is published on.
///
/// A `##` heading owns a page; the `###` and `####` headings under it are
/// fragments of that page. `DOCS.md` publishes under `stdlib/`, `LANGUAGE.md`
/// under `language/` — except for its two reference sections, which the
/// website routes elsewhere, so those are named explicitly rather than
/// guessed.
fn collect_anchors() -> HashMap<String, String> {
    let mut map = HashMap::new();
    for (src, area) in [(DOCS, "stdlib"), (LANGUAGE, "language")] {
        let mut page = String::new();
        let mut in_fence = false;
        for line in src.lines() {
            if line.trim_start().starts_with("```") {
                in_fence = !in_fence;
                continue;
            }
            if in_fence {
                continue;
            }
            // `#### `String.sub(…)`` anchors under the name alone. A `####`
            // heading that is not a backticked signature falls through to the
            // generic handling below, which slugifies its text.
            if let Some(sig) = line
                .strip_prefix("#### ")
                .map(str::trim)
                .and_then(|h| h.strip_prefix('`'))
                .and_then(|h| h.strip_suffix('`'))
                .filter(|_| !page.is_empty())
            {
                let a = signature_anchor(sig);
                if !a.is_empty() {
                    let url = format!("{DOCS_BASE}/{page}/#{a}");
                    map.entry(a).or_insert(url);
                }
                continue;
            }
            let Some((hashes, title)) = line
                .split_once(' ')
                .filter(|(h, _)| !h.is_empty() && h.len() <= 4 && h.chars().all(|c| c == '#'))
            else {
                continue;
            };
            let title = plain_text(title);
            let slug = slugify(&title);
            if slug.is_empty() {
                continue;
            }
            if hashes.len() == 2 {
                // `Io` / `File` publishes as `io-file`; the slug already
                // collapses the separator.
                page = format!("{area}/{slug}");
                // The website routes these two out of the guide.
                if area == "language" && matches!(slug.as_str(), "quick-reference" | "grammar") {
                    page = format!("reference/{slug}");
                }
                map.insert(slug, format!("{DOCS_BASE}/{page}/"));
            } else if !page.is_empty() {
                // A sub-heading is a fragment of its page, so the anchor has
                // to survive into the URL — without it every link into a
                // section lands at the top of the page it lives on.
                let url = format!("{DOCS_BASE}/{page}/#{slug}");
                map.entry(slug).or_insert(url);
            }
        }
    }
    map
}

/// An instance method is documented on a sample receiver, because that is how
/// it is called. The index wants the type, since nothing is ever typed `f`.
fn receiver_type(name: &str) -> Option<&'static str> {
    match name {
        "f" => Some("File"),
        "p" => Some("Process"),
        "gen" => Some("Coroutine"),
        "co" => Some("Yielder"),
        _ => None,
    }
}

/// GitHub's heading-slug algorithm, which is what `DOCS.md`'s own links and
/// the website's routes are written against: lowercase, drop anything that is
/// not a word character, space or hyphen, then spaces to hyphens.
fn slugify(heading: &str) -> String {
    let lowered = heading.trim().to_lowercase();
    let kept: String = lowered
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || c.is_whitespace() || *c == '-')
        .collect();
    let mut out = String::with_capacity(kept.len());
    let mut in_space = false;
    for c in kept.chars() {
        if c.is_whitespace() {
            in_space = true;
            continue;
        }
        if in_space && !out.is_empty() {
            out.push('-');
        }
        in_space = false;
        out.push(c);
    }
    out
}

/// Strip the inline Markdown a heading may carry, so `` ## `String` ``
/// yields `String`.
fn plain_text(md: &str) -> String {
    md.replace('`', "").replace("**", "").trim().to_string()
}

/// `String.sub(…)` → `string-sub`, the anchor the reference publishes.
fn signature_anchor(sig: &str) -> String {
    let head: String = sig
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.')
        .collect();
    slugify(&head.replace('.', " "))
}

/// Split a documented signature into the pieces the editor needs.
///
/// Returns `None` for a heading that is not a signature at all, which is how
/// `#### Operators` and `#### A worked example` are filtered out.
struct ParsedSig {
    owner: Option<String>,
    name: String,
    params: Vec<String>,
    is_method: bool,
}

fn parse_signature(sig: &str) -> Option<ParsedSig> {
    let open = sig.find('(')?;
    // `Table.insert<V>(…)` — the type parameters sit between the name and the
    // parameter list and belong to neither.
    let mut head = &sig[..open];
    if let Some(lt) = head.find('<') {
        head = &head[..lt];
    }
    let head = head.trim();
    let (raw_owner, name) = match head.rfind('.') {
        Some(dot) => (Some(&head[..dot]), &head[dot + 1..]),
        None => (None, head),
    };
    // A name that is not an identifier means this heading is prose.
    if name.is_empty()
        || !name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return None;
    }
    let is_method = raw_owner.is_some_and(|o| receiver_type(o).is_some());
    let owner = raw_owner.map(|o| receiver_type(o).unwrap_or(o).to_string());

    // The parameter list ends at the paren closing the one just opened.
    let mut depth = 0usize;
    let mut close = None;
    for (i, c) in sig.char_indices().skip_while(|(i, _)| *i < open) {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;
    let params = split_params(&sig[open + 1..close]);
    Some(ParsedSig {
        owner,
        name: name.to_string(),
        params,
        is_method,
    })
}

/// Parameter *names* from a documented parameter list.
///
/// Splits on top-level commas — a nested `fn(a, b) -> c` or `table<K, V>`
/// carries its own — then takes the name before each `:`. A variadic tail
/// (`...any`) has no name and is called after its type, which is what
/// `saule-lsp` names it when it has to guess.
fn split_params(list: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut current = String::new();
    for c in list.chars() {
        match c {
            '(' | '<' | '[' | '{' => {
                depth += 1;
                current.push(c);
            }
            ')' | '>' | ']' | '}' => {
                depth = depth.saturating_sub(1);
                current.push(c);
            }
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut current));
            }
            _ => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        out.push(current);
    }
    out.iter()
        .filter_map(|raw| {
            let p = raw.trim();
            if p.is_empty() {
                return None;
            }
            // A variadic declares a type, never a name — see `params`.
            if p.starts_with("...") {
                return None;
            }
            // `s: string` → `s`. A bare type with no name keeps the type.
            let name = match p.find(':') {
                Some(colon) => p[..colon].trim(),
                None => p,
            };
            Some(name.to_string())
        })
        .collect()
}

/// The first paragraph of prose, which is what fits a completion popup.
fn first_paragraph(body: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for line in body.lines() {
        let t = line.trim();
        if t.starts_with("```") {
            break;
        }
        if t.is_empty() {
            if !out.is_empty() {
                break;
            }
            continue;
        }
        // A table or list starts new structure, not more of this paragraph.
        if t.starts_with('|') || t.starts_with("- ") {
            if !out.is_empty() {
                break;
            }
            continue;
        }
        out.push(t);
    }
    out.join(" ")
}

fn parse(src: &str) -> StdlibDocs {
    let mut docs = StdlibDocs::default();

    // Section state as the scan walks down the file.
    let mut module: Option<String> = None;
    let mut module_url = String::new();
    let mut group: Option<String> = None;
    // The heading whose body is currently accumulating, and that body.
    let mut pending: Option<Heading> = None;
    let mut buf: Vec<&str> = Vec::new();
    let mut in_fence = false;

    enum Heading {
        Module { names: Vec<String>, url: String },
        Entry(Box<EntryHead>),
    }
    struct EntryHead {
        signature: String,
        parsed: ParsedSig,
        group: Option<String>,
        url: String,
    }

    // Flushing is what turns the accumulated lines into an entry or a module.
    macro_rules! flush {
        () => {
            if let Some(h) = pending.take() {
                let body = buf.join("\n").trim().to_string();
                let summary = first_paragraph(&body);
                match h {
                    Heading::Module { names, url } => {
                        for name in names {
                            docs.modules.insert(
                                name.clone(),
                                StdlibModule {
                                    name,
                                    summary: summary.clone(),
                                    body: body.clone(),
                                    url: url.clone(),
                                },
                            );
                        }
                    }
                    Heading::Entry(e) => {
                        let qname = match &e.parsed.owner {
                            Some(o) => format!("{o}.{}", e.parsed.name),
                            None => e.parsed.name.clone(),
                        };
                        // An overload set documents the same name more than
                        // once (`Math.random()`, `Math.random(lo, hi)`).
                        // First one wins and the rest are folded into its
                        // body, so the hover shows every form.
                        if let Some(existing) = docs.entries.get_mut(&qname) {
                            existing.body.push_str("\n\n---\n\n`");
                            existing.body.push_str(&e.signature);
                            existing.body.push_str("`\n\n");
                            existing.body.push_str(&body);
                        } else {
                            docs.entries.insert(
                                qname.clone(),
                                StdlibEntry {
                                    qname,
                                    owner: e.parsed.owner,
                                    name: e.parsed.name,
                                    signature: e.signature,
                                    params: e.parsed.params,
                                    summary,
                                    body,
                                    group: e.group,
                                    url: e.url,
                                    is_method: e.parsed.is_method,
                                    kind: EntryKind::Function,
                                },
                            );
                        }
                    }
                }
            }
            buf.clear();
        };
    }

    for line in src.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            if pending.is_some() {
                buf.push(line);
            }
            continue;
        }
        if in_fence {
            if pending.is_some() {
                buf.push(line);
            }
            continue;
        }

        if let Some(rest) = line
            .strip_prefix("## ")
            .filter(|_| !line.starts_with("###"))
        {
            flush!();
            let title = plain_text(rest);
            // `Table of Contents` and `Conventions` are prose, not modules;
            // they are recognised by having no entries rather than by name,
            // so they are allowed in and simply never looked up.
            let slug = slugify(&title);
            let url = format!("{DOCS_BASE}/stdlib/{slug}/");
            // `Io / File` documents two names on one page.
            let names: Vec<String> = title
                .split('/')
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty() && n.chars().next().is_some_and(char::is_uppercase))
                .collect();
            module = names.first().cloned();
            module_url = url.clone();
            group = None;
            if !names.is_empty() {
                pending = Some(Heading::Module { names, url });
            }
            continue;
        }

        if let Some(rest) = line
            .strip_prefix("### ")
            .filter(|_| !line.starts_with("####"))
        {
            flush!();
            group = Some(plain_text(rest));
            continue;
        }

        if let Some(rest) = line.strip_prefix("#### ") {
            flush!();
            let heading = rest.trim();
            // An API entry's heading is a backticked signature. Anything else
            // (`#### Operators`) is prose structure.
            let Some(sig) = heading
                .strip_prefix('`')
                .and_then(|h| h.strip_suffix('`'))
                .filter(|_| module.is_some())
            else {
                continue;
            };
            if let Some(parsed) = parse_signature(sig) {
                let anchor = signature_anchor(sig);
                pending = Some(Heading::Entry(Box::new(EntryHead {
                    signature: sig.to_string(),
                    parsed,
                    group: group.clone(),
                    url: format!("{module_url}#{anchor}"),
                })));
            }
            continue;
        }

        // A constant is a row in a module's table rather than a heading of
        // its own: `| `Math.pi` | `float` | π |`. Collected as the scan
        // passes it, because it has no body to accumulate.
        if let Some(c) = parse_constant_row(line, &module_url) {
            docs.entries.insert(c.qname.clone(), c);
            // Fall through: the row is also part of the enclosing body.
        }

        if pending.is_some() {
            buf.push(line);
        }
    }
    flush!();
    docs
}

/// A documented constant, from a table row in a module section.
///
/// The row shape is `| `Qname` | `type` | description |`, which every module
/// that has constants uses. A row that is not that — a header, a separator,
/// or a table about something else — yields `None`.
fn parse_constant_row(line: &str, module_url: &str) -> Option<StdlibEntry> {
    let t = line.trim();
    if !t.starts_with('|') {
        return None;
    }
    let cells: Vec<&str> = t.trim_matches('|').split('|').map(str::trim).collect();
    if cells.len() < 3 {
        return None;
    }
    // Cell 1 is a backticked qualified name, cell 2 a backticked type.
    let qname = cells[0].strip_prefix('`')?.strip_suffix('`')?;
    let ty = cells[1].strip_prefix('`')?.strip_suffix('`')?;
    let (owner, name) = qname.split_once('.')?;
    if ty.is_empty()
        || qname.contains('(')
        || !owner.chars().next().is_some_and(char::is_uppercase)
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        || name.is_empty()
    {
        return None;
    }
    // The description may itself hold `|`-free Markdown; the remaining cells
    // are rejoined in case a description contained an escaped pipe.
    let desc = cells[2..].join(" | ").trim().to_string();
    if desc.is_empty() {
        return None;
    }
    let anchor = signature_anchor(qname);
    Some(StdlibEntry {
        qname: qname.to_string(),
        owner: Some(owner.to_string()),
        name: name.to_string(),
        signature: format!("{qname}: {ty}"),
        params: Vec::new(),
        summary: desc.clone(),
        body: desc,
        group: None,
        url: format!("{module_url}#{anchor}"),
        is_method: false,
        kind: EntryKind::Constant,
    })
}

#[cfg(test)]
mod stdlib_tests {
    use super::*;

    #[test]
    fn the_index_covers_the_library() {
        let d = stdlib();
        // A floor, not an exact count: adding a function should not fail this.
        assert!(
            d.entry_count() > 140,
            "only {} entries parsed — the DOCS.md heading shape probably changed",
            d.entry_count()
        );
        assert!(d.module_count() >= 9, "{} modules", d.module_count());
    }

    #[test]
    fn a_static_entry_carries_names_and_prose() {
        let e = stdlib().entry("String.sub").expect("String.sub documented");
        assert_eq!(e.owner.as_deref(), Some("String"));
        assert_eq!(e.name, "sub");
        assert_eq!(e.params, vec!["s", "from", "to"]);
        assert!(!e.summary.is_empty(), "summary should be prose");
        assert!(e.summary.len() > 20, "summary too short: {:?}", e.summary);
        assert_eq!(e.group.as_deref(), Some("Length and slicing"));
        assert_eq!(
            e.url,
            "https://lauriszz123.github.io/saule/stdlib/string/#string-sub"
        );
        assert!(!e.is_method);
    }

    #[test]
    fn the_new_pattern_functions_are_indexed() {
        for (qname, params) in [
            ("String.match", vec!["s", "pat", "from"]),
            ("String.gmatch", vec!["s", "pat"]),
            ("String.gsub", vec!["s", "pat", "repl", "limit"]),
        ] {
            let e = stdlib().entry(qname).unwrap_or_else(|| panic!("{qname}"));
            assert_eq!(e.params, params, "{qname}");
            assert_eq!(e.group.as_deref(), Some("Patterns"), "{qname}");
            assert!(!e.summary.is_empty(), "{qname} has no summary");
        }
    }

    #[test]
    fn a_prelude_entry_has_no_owner() {
        let e = stdlib().entry("println").expect("println documented");
        assert!(e.owner.is_none());
        assert!(
            e.params.is_empty(),
            "`println(...any)` has no named parameters, only a variadic: {:?}",
            e.params
        );
        assert!(!e.summary.is_empty());
    }

    #[test]
    fn a_generic_entry_keeps_its_parameters() {
        // `assert<T>(v: T?, msg: string?) -> T` — the `<T>` belongs to
        // neither the name nor the parameter list.
        let e = stdlib().entry("assert").expect("assert documented");
        assert_eq!(e.name, "assert");
        assert_eq!(e.params, vec!["v", "msg"]);
    }

    #[test]
    fn an_instance_method_is_keyed_by_its_type() {
        // Documented as `f.read`, indexed as `File.read`.
        let e = stdlib().entry("File.read").expect("File.read documented");
        assert!(e.is_method);
        assert_eq!(e.owner.as_deref(), Some("File"));
        assert!(e.url.contains("/stdlib/io-file/#f-read"), "{}", e.url);
    }

    #[test]
    fn a_nested_function_type_is_one_parameter() {
        // `Table.sort<V>(t: table<V>, cmp: fn(V, V) -> boolean)` — the comma
        // inside `fn(V, V)` must not split the list.
        let e = stdlib().entry("Table.sort").expect("Table.sort documented");
        assert_eq!(e.params, vec!["t", "cmp"], "sig: {}", e.signature);
    }

    #[test]
    fn modules_carry_their_own_prose() {
        let m = stdlib().module("String").expect("String module");
        assert!(!m.summary.is_empty());
        assert_eq!(m.url, "https://lauriszz123.github.io/saule/stdlib/string/");
        // `Io / File` documents two names on one page.
        assert!(stdlib().module("Io").is_some());
        assert!(stdlib().module("File").is_some());
    }

    #[test]
    fn prose_headings_are_not_entries() {
        // `#### Operators`, `#### A worked example` and friends are structure.
        for not_an_entry in ["Operators", "Everything else", "A worked example"] {
            assert!(
                stdlib().entry(not_an_entry).is_none(),
                "{not_an_entry} should not be an API entry"
            );
        }
    }

    #[test]
    fn every_entry_is_well_formed() {
        for e in stdlib().all() {
            assert!(!e.name.is_empty(), "{}", e.qname);
            assert!(!e.signature.is_empty(), "{}", e.qname);
            assert!(
                !e.summary.trim().is_empty(),
                "{} has no summary — every documented function needs one",
                e.qname
            );
            assert!(e.url.starts_with(DOCS_BASE), "{}: {}", e.qname, e.url);
            // The signature should mention the name it is keyed by.
            assert!(
                e.signature.contains(&e.name),
                "{} signature {:?}",
                e.qname,
                e.signature
            );
        }
    }

    #[test]
    fn constants_come_from_the_module_tables() {
        for (qname, sig) in [
            ("Math.pi", "Math.pi: float"),
            ("Math.maxinteger", "Math.maxinteger: integer"),
            ("Os.sep", "Os.sep: string"),
            ("Saule.version", "Saule.version: string"),
            ("Saule.isDev", "Saule.isDev: boolean"),
            ("Project.root", "Project.root: string"),
            ("Project.srcDirs", "Project.srcDirs: table<string>"),
        ] {
            let e = stdlib().entry(qname).unwrap_or_else(|| panic!("{qname}"));
            assert_eq!(e.kind, EntryKind::Constant, "{qname}");
            assert_eq!(e.signature, sig, "{qname}");
            assert!(e.params.is_empty(), "{qname} is not callable");
            assert!(!e.summary.is_empty(), "{qname} has no description");
        }
    }

    #[test]
    fn a_table_that_is_not_a_constant_list_is_ignored() {
        // The pattern-syntax tables in the `String` section have cells like
        // `%d` and `.` in the first column, and a class table's header row is
        // `| Class | Matches |`. None of those is a qualified name.
        for not_a_constant in ["%d", ".", "Class", "Name", "Item", "Quantifier"] {
            assert!(
                stdlib().entry(not_a_constant).is_none(),
                "{not_a_constant} should not be an entry"
            );
        }
        // And no entry should have arrived with a lowercase owner.
        for e in stdlib().all() {
            if let Some(o) = &e.owner {
                assert!(
                    o.chars().next().is_some_and(char::is_uppercase),
                    "{} has owner {o:?}",
                    e.qname
                );
            }
        }
    }

    #[test]
    fn functions_and_constants_are_distinguished() {
        assert_eq!(
            stdlib().entry("Math.floor").map(|e| e.kind),
            Some(EntryKind::Function)
        );
        assert_eq!(
            stdlib().entry("Math.pi").map(|e| e.kind),
            Some(EntryKind::Constant)
        );
        // `Math.huge` is documented in the constant table *and* has a
        // `tests/errors` fixture for calling it, so it must stay a constant.
        assert_eq!(
            stdlib().entry("Math.huge").map(|e| e.kind),
            Some(EntryKind::Constant)
        );
    }

    #[test]
    fn members_of_finds_a_modules_functions() {
        let m = stdlib().members_of("String");
        assert!(m.len() > 20, "{} String members", m.len());
        assert!(m.iter().any(|e| e.name == "match"));
        assert!(m.iter().any(|e| e.name == "gsub"));
        // Sorted, so a rendered list is stable.
        let names: Vec<&str> = m.iter().map(|e| e.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
    }
}
