#!/usr/bin/env node
/**
 * Split the repo's long-form Markdown into Starlight pages.
 *
 * LANGUAGE.md and DOCS.md are the canonical prose for the language and its
 * standard library. Rather than fork them into the website — where the copy
 * would rot the first time a feature lands — this script slices them on `##`
 * boundaries into one page per section and rewrites the cross-references.
 *
 * README.md is deliberately *not* a source here: it is the repository's front
 * door, a short page of install-and-orient that links to both of these. The
 * language guide used to live in it, which made the front door 3,900 lines
 * long.
 *
 * The hard part is links. Inside a single document, `[classes](#classes)` is
 * an in-page anchor; once `## Classes` becomes its own route, that anchor has
 * to become `/saule/language/classes/`. So the script first indexes *every*
 * heading in both files (h2 and h3, since some links target h3s like
 * `#escaping-any-with-as`), then rewrites each `](#...)` against that index.
 *
 * Run it with `npm run sync-docs` from `www/`. Output goes to
 * `src/content/docs/{language,stdlib,reference}` and is overwritten wholesale
 * — never hand-edit those files; edit LANGUAGE.md/DOCS.md instead.
 */
import { readFileSync, writeFileSync, mkdirSync, rmSync, existsSync, readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { withBase, repo } from '../site.config.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, '..', '..');
const docsRoot = join(here, '..', 'src', 'content', 'docs');

/**
 * GitHub's heading-slug algorithm, which is what the existing `](#...)`
 * links in LANGUAGE.md were written against. Lowercase, strip anything that
 * isn't a word character/space/hyphen, spaces to hyphens. Backticks and
 * parens vanish, which is why `### Folder Modules (\`init.sau\`)` is linked
 * as `#folder-modules-initsau`.
 */
function slugify(heading) {
	return heading
		.trim()
		.toLowerCase()
		.replace(/[^\w\s-]/g, '')
		.replace(/\s+/g, '-');
}

/**
 * Split a Markdown document into `##` sections, ignoring `#` headings that
 * sit inside fenced code blocks (Saule comments never start with `#`, but
 * shell fences in the install docs do).
 */
function splitSections(markdown) {
	const lines = markdown.split('\n');
	const sections = [];
	let current = null;
	let inFence = false;

	for (const line of lines) {
		if (/^\s*```/.test(line)) inFence = !inFence;

		const h2 = !inFence && /^## (.+)$/.exec(line);
		if (h2) {
			if (current) sections.push(current);
			current = { title: h2[1].trim(), lines: [] };
			continue;
		}
		if (current) current.lines.push(line);
	}
	if (current) sections.push(current);

	return sections.map((s) => ({ ...s, body: s.lines.join('\n').trim() }));
}

/** Collect every h2/h3 slug in a document, in order. */
function headingsOf(markdown) {
	const out = [];
	let inFence = false;
	for (const line of markdown.split('\n')) {
		if (/^\s*```/.test(line)) inFence = !inFence;
		if (inFence) continue;
		// A per-function entry (`#### \`String.sub(...)\``) anchors under the
		// name alone, matching the id `renderApiEntries` gives it — so a
		// `[...](#string-sub)` written anywhere in the source resolves to the
		// page that function actually lives on.
		const api = /^#### `([^`]+)`\s*$/.exec(line);
		if (api) {
			out.push({ level: 4, text: api[1], slug: signatureAnchor(api[1]) });
			continue;
		}
		const m = /^(#{2,3}) (.+)$/.exec(line);
		if (m) out.push({ level: m[1].length, text: m[2].trim(), slug: slugify(m[2]) });
	}
	return out;
}

/**
 * Strip inline Markdown so a heading can be used as a frontmatter title and
 * as sidebar text: `` `nil` Is a Value `` → `nil Is a Value`.
 */
function plainText(md) {
	return md
		.replace(/`([^`]*)`/g, '$1')
		.replace(/\*\*([^*]*)\*\*/g, '$1')
		.replace(/\[([^\]]*)\]\([^)]*\)/g, '$1')
		.trim();
}

/** YAML-safe scalar: always quote, escape embedded quotes and backslashes. */
function yamlString(value) {
	return `"${value.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"`;
}

/**
 * A `tree/main/...` URL into the repository, with each path segment percent-
 * encoded.
 *
 * The "Also in the repository" list is built from `readdirSync`, so the
 * segments are whatever the directories are actually called — and
 * `examples/UI Project` produced a link with a raw space in it, which stops
 * being a link at all once a Markdown parser reaches the space.
 *
 * Separators are preserved: only the segments between them are encoded.
 */
function repoUrl(path) {
	const encoded = path.split('/').map(encodeURIComponent).join('/');
	return `${repo}/tree/main/${encoded}`;
}

/**
 * A path as it should appear inside a `sh` code block. Anything the shell
 * would split on gets single-quoted, so the `cd` line in "Run it" still works
 * when copied verbatim.
 */
function shellPath(path) {
	return /^[A-Za-z0-9._\-/]+$/.test(path) ? path : `'${path.replace(/'/g, `'\\''`)}'`;
}

/**
 * Pull the first real prose paragraph out of a section body, for use as the
 * page description (feeds `<meta name="description">` and search results).
 */
function firstParagraph(body) {
	let inFence = false;
	const buf = [];
	for (const line of body.split('\n')) {
		if (/^\s*```/.test(line)) {
			if (buf.length) break;
			inFence = !inFence;
			continue;
		}
		if (inFence) continue;
		const t = line.trim();
		if (!t) {
			if (buf.length) break;
			continue;
		}
		// Skip structural lines — headings, tables, list bullets, quotes.
		if (/^[#>|\-*]/.test(t) || /^\d+\./.test(t)) {
			if (buf.length) break;
			continue;
		}
		buf.push(t);
	}
	const text = plainText(buf.join(' '));
	if (!text) return undefined;
	return text.length > 155 ? `${text.slice(0, 152).replace(/\s+\S*$/, '')}…` : text;
}

/**
 * Build the anchor index: every heading slug in the source documents mapped
 * to the route + fragment it ends up at on the website.
 */
function buildAnchorIndex(plans) {
	const index = new Map();
	for (const plan of plans) {
		for (const section of plan.sections) {
			// `section.dir`, not `plan.dir`: a rerouted section (see REROUTE)
			// is written somewhere other than its document's default directory,
			// and a link to it has to point where the page actually landed.
			const route = withBase(`/${section.dir}/${section.fileSlug}/`);
			// The h2 itself becomes the page, so it has no fragment.
			index.set(section.sourceSlug, route);
			for (const h of headingsOf(section.body)) {
				// h3s stay as fragments within their page. First writer wins, so
				// a duplicate sub-heading elsewhere can't hijack an existing link.
				if (!index.has(h.slug)) index.set(h.slug, `${route}#${h.slug}`);
			}
		}
	}
	return index;
}

/**
 * Rewrite links for the website:
 *   `](#classes)`               → `](/saule/language/classes/)`
 *   `](./DOCS.md#table)`        → `](/saule/stdlib/table/)`
 *   `](./DOCS.md)`              → `](/saule/stdlib/prelude/)`
 *   `](./LANGUAGE.md#...)`      → the matching language page
 *
 * Both `./` and `../` prefixes are accepted. DOCS.md's own prose reaches the
 * language guide as `../LANGUAGE.md#...` in places, and when only `./` was
 * matched those links were left verbatim in the generated page — a relative
 * path to a repository file, which is a 404 on the site.
 *
 * Anything unresolved is reported so a broken link surfaces at sync time
 * rather than as a 404 in production.
 */
function rewriteLinks(body, index, { file, section }) {
	const unresolved = [];

	const resolve = (anchor) => {
		const target = index.get(anchor);
		if (!target) unresolved.push(anchor);
		return target;
	};

	let out = body.replace(
		/\]\((\.{1,2}\/(?:LANGUAGE|DOCS)\.md)?#([^)\s]+)\)/g,
		(match, _doc, anchor) => {
			const target = resolve(anchor);
			return target ? `](${target})` : match;
		},
	);

	// Bare document links, with no fragment, land on each doc's first page.
	out = out
		.replace(/\]\(\.{1,2}\/DOCS\.md\)/g, `](${withBase('/stdlib/prelude/')})`)
		.replace(/\]\(\.{1,2}\/LANGUAGE\.md\)/g, `](${withBase('/guides/introduction/')})`)
		.replace(/\]\(\.{1,2}\/README\.md\)/g, `](${withBase('/guides/introduction/')})`);

	// `./examples/<dir>` is the project directory when the guide is read on
	// GitHub. On the site it becomes that example's generated page — or a
	// repository link when the project has no page, which is a 404 otherwise.
	out = out.replace(/\]\(\.{1,2}\/examples\/([^)\s]+?)\/?\)/g, (match, dir) =>
		EXAMPLES.some((e) => e.dir === dir)
			? `](${withBase(`/examples/${dir}/`)})`
			: `](${repoUrl(`examples/${dir}`)})`,
	);

	for (const anchor of unresolved) {
		console.warn(`  ! ${file} › ${section}: unresolved link target #${anchor}`);
	}
	return out;
}

/**
 * Sections that shouldn't become pages: a hand-maintained table of contents
 * is redundant once Starlight generates the sidebar.
 *
 * "Quick Reference" and "Grammar" are reference material rather than parts of
 * the language guide — they are things you come back to, not things you read
 * once — so they are routed out of `language/` into `reference/`.
 */
const SKIP = new Set(['Table of Contents']);
const REROUTE = new Map([
	['Quick Reference', { dir: 'reference', fileSlug: 'quick-reference' }],
	['Grammar', { dir: 'reference', fileSlug: 'grammar' }],
]);

/** Explicit sidebar ordering — teaching order, not the order of the guide. */
const ORDER = {
	language: [
		'types',
		'variables',
		'tables',
		'functions',
		'lambdas-and-closures',
		'classes',
		'interfaces',
		'enums',
		'pattern-matching',
		'null-safety',
		'error-handling',
		'loops',
		'coroutines',
		'imports-and-file-structure',
		'project-configuration',
	],
	stdlib: [
		'prelude-always-in-scope',
		'string',
		'bytes',
		'math',
		'table',
		'iter',
		'coroutine',
		'io-file',
		'os',
		'saule',
		'project',
		'conventions',
	],
};

/**
 * Turn ```luau fences back into ```saule.
 *
 * LANGUAGE.md and DOCS.md are read on GitHub first, and GitHub highlights a
 * fence only when Linguist knows the language — it has no Saule entry, so
 * ```saule renders as flat grey text there. The sources therefore tag their
 * blocks `luau`, the nearest language Linguist *does* know (Lua plus `:`
 * annotations, `->`, `?` and `<T>`), and this script swaps the tag back on the
 * way into the site, where the real grammar is registered.
 *
 * Only the tag is touched; anything after it — Expressive Code's `title=`,
 * line markers — is preserved.
 */
function restoreSauleFences(body) {
	return body.replace(/^(\s*`{3,})luau\b/gm, '$1saule');
}

function planDocument({ file, dir, sourcePath }) {
	const markdown = readFileSync(sourcePath, 'utf8');
	const sections = splitSections(markdown)
		.filter((s) => !SKIP.has(plainText(s.title)))
		.map((s) => {
			const sourceSlug = slugify(s.title);
			const reroute = REROUTE.get(plainText(s.title));
			return {
				...s,
				sourceSlug,
				fileSlug: reroute?.fileSlug ?? sourceSlug,
				dir: reroute?.dir ?? dir,
			};
		});
	return { file, dir, sections };
}

// ---------------------------------------------------------------------------
// API tables
// ---------------------------------------------------------------------------

/**
 * The standard library's reference tables are the pages people *use*, and a
 * two-column table of inline code is the wrong shape for them: the signature
 * wraps mid-type, nothing is highlighted, and every description is expanded
 * at once whether or not it is the one you came for.
 *
 * These turn each row into an expandable entry with a highlighted signature.
 * Done here, at generation time, rather than in the browser: the markup is
 * static, there is no flash of unstyled table, and the generated pages stay
 * plain `.md` — they are lifted verbatim from `DOCS.md`, so a stray `{` must
 * not be able to break a build.
 */

/** Type and keyword names the signatures use, longest first so `bytes` wins over `byte`. */
const SIG_TYPES = [
	'integer', 'boolean', 'string', 'bytes', 'float', 'table', 'byte', 'any', 'nil', 'fn',
	'File', 'Process', 'FsInfo', 'FsKind', 'IoMode', 'IoSeek', 'OsPlatform',
];

/**
 * Colour a signature the way the code blocks on the same page are coloured.
 *
 * A scanner rather than a chain of replacements: replacing names inside text
 * that already holds `<span>` tags corrupts the tags themselves, and the
 * order the patterns run in silently decides the result.
 */
function highlightSignature(sig, { code = false } = {}) {
	const token = new RegExp(
		[
			'(?<comment>--[^\\n]*)',
			'(?<str>"(?:[^"\\\\]|\\\\.)*")',
			'(?<num>\\b0[xX][0-9a-fA-F]+\\b|\\b\\d+(?:\\.\\d+)?\\b)',
			'(?<kw>\\b(?:true|false|nil|local|fn|return|end|if|then|else|for|in|do|not|and|or)\\b)',
			code
				? '(?<fn>\\b[A-Za-z_][\\w]*(?:\\.[A-Za-z_][\\w]*)?(?=\\())'
				: '(?<fn>^[A-Za-z_][\\w]*\\.[A-Za-z_][\\w]*|^[a-z_][\\w]*(?=\\())',
			`(?<type>\\b(?:${SIG_TYPES.join('|')})\\b|\\b[A-Z][A-Za-z0-9]*\\b)`,
			'(?<arrow>->|==|!=|<=|>=|\\?\\?)',
			'(?<punct>[(),:?<>\\[\\]{}|=+*/%#!-]|\\.\\.\\.)',
			'(?<name>[A-Za-z_][\\w]*)',
			'(?<other>\\s+|.)',
		].join('|'),
		'gu'
	);
	let out = '';
	for (const m of sig.matchAll(token)) {
		const g = m.groups;
		const text = escapeHtml(m[0]);
		if (g.comment) out += `<span class="sig-comment">${text}</span>`;
		else if (g.str) out += `<span class="sig-str">${text}</span>`;
		else if (g.num) out += `<span class="sig-num">${text}</span>`;
		else if (g.kw) out += `<span class="sig-kw">${text}</span>`;
		else if (g.fn) out += `<span class="sig-fn">${text}</span>`;
		else if (g.type) out += `<span class="sig-type">${text}</span>`;
		else if (g.arrow) out += `<span class="sig-arrow">${text}</span>`;
		else if (g.punct) out += `<span class="sig-punct">${text}</span>`;
		else if (g.name) out += `<span class="sig-param">${text}</span>`;
		else out += text;
	}
	return out;
}

function escapeHtml(text) {
	return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

/**
 * The slice of markdown these descriptions actually use — code spans, links,
 * bold and italics. Raw HTML suspends markdown parsing, so once a row is an
 * HTML block its description has to arrive already rendered.
 */
function renderInline(md) {
	const code = [];
	// Code spans first and stashed, so a `*` or `_` inside one is not read as
	// emphasis and a `<` inside one is escaped exactly once.
	let out = md.replace(/`([^`]+)`/g, (_, inner) => {
		code.push(`<code>${escapeHtml(inner)}</code>`);
		return `\u0000${code.length - 1}\u0000`;
	});
	out = escapeHtml(out);
	out = out.replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, '<a href="$2">$1</a>');
	out = out.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
	out = out.replace(/(^|[^*])\*([^*]+)\*/g, '$1<em>$2</em>');
	return out.replace(/\u0000(\d+)\u0000/g, (_, i) => code[Number(i)]);
}

/**
 * Pull the worked examples out of a description.
 *
 * They are written as a trailing sentence of inline code — `` `expr == v` ``
 * — which reads correctly in `DOCS.md` on GitHub, where there is no room for
 * a code block inside a table cell. Here there *is* room, so they are lifted
 * out and rendered as one, syntax-highlighted like any other Saule code on
 * the page. What is left behind is the prose.
 */
function splitExamples(desc) {
	// Only the closing sentence, and only when it reads as a claim about a
	// result — `x == y`, or `expr` is `value`. A sentence that merely
	// *mentions* a function ("Same operation as `Table.concat(t, sep)`")
	// says nothing worth running, and lifting it would turn prose into a
	// code block that proves nothing.
	const sentences = desc.split(/(?<=\.)\s+/);
	const last = sentences[sentences.length - 1] ?? '';
	if (!/==|\sis\s/.test(last)) return { prose: desc, examples: [] };

	const examples = [];
	let tail = last.replace(/`([^`]+)`/g, (match, inner) => {
		// An expression, not a name: it calls something, indexes something,
		// or is a literal being compared against one.
		if (!/[(#{]/.test(inner) && !/^-?\d/.test(inner)) return match;
		examples.push(inner);
		return '\u0000';
	});
	if (examples.length === 0) return { prose: desc, examples: [] };

	// What is left of that sentence is connective tissue — ", and", "is 6." —
	// and reads as noise once the code is a block of its own.
	tail = tail.replace(/\u0000/g, '').replace(/\s+/g, ' ').trim();
	const keep = /^[,.;:]?\s*(?:and|while|,|is|where|—|-|\.|\s|[a-z0-9 ]{0,24}\.)?$/i.test(tail)
		? ''
		: tail;

	const prose = [...sentences.slice(0, -1), keep]
		.join(' ')
		.replace(/\s+/g, ' ')
		.replace(/[\s:,]+$/, '')
		.trim();
	return { prose: prose.endsWith('.') || prose === '' ? prose : `${prose}.`, examples };
}

/** `String.sub(...)` -> `string-sub`, for a stable anchor. */
function signatureAnchor(sig) {
	const name = sig.match(/^([A-Za-z_][\w]*(?:\.[A-Za-z_][\w]*)?)/)?.[1] ?? sig;
	return name.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');
}

/**
 * Turn each `#### \`signature\`` block into an expandable entry.
 *
 * This is the shape the reference is written in: one heading per function,
 * carrying its own prose and its own runnable example. The heading is
 * replaced by a `<summary>` so the page collapses to a scannable list of
 * signatures, and everything under it is handed back to the markdown parser
 * untouched — links, lists, and fenced blocks that Expressive Code
 * highlights with the project's own grammar.
 */
function renderApiEntries(body) {
	const lines = body.split('\n');
	const out = [];
	let open = false;
	// An overload set — `Math.random()`, `Math.random(n)`, `Math.random(lo, hi)`
	// — gives every entry the same name, and three elements sharing one `id`
	// is invalid HTML whose anchor links all land on the first of them.
	const used = new Map();
	const uniqueAnchor = (sig) => {
		const base = signatureAnchor(sig);
		const n = (used.get(base) ?? 0) + 1;
		used.set(base, n);
		return n === 1 ? base : `${base}-${n}`;
	};
	const close = () => {
		if (open) {
			// The trailing blank line is load-bearing: without it the next
			// line continues the HTML block, and a `### heading` right after
			// a `</div>` renders as the literal text `### heading`.
			out.push('', '</details>', '</div>', '');
			open = false;
		}
	};
	for (let i = 0; i < lines.length; i++) {
		const heading = lines[i].match(/^####\s+`([^`]+)`\s*$/);
		// Any other heading ends the run: a `###` is a new group of
		// functions, and its own prose must not land inside the last entry.
		if (!heading && /^#{1,3}\s/.test(lines[i])) {
			close();
			out.push(lines[i]);
			continue;
		}
		if (!heading) {
			out.push(lines[i]);
			continue;
		}
		close();
		const sig = heading[1];
		out.push('<div class="api-list">');
		out.push(`<details class="api-entry" id="${uniqueAnchor(sig)}">`);
		out.push(`<summary><code class="api-sig">${highlightSignature(sig)}</code></summary>`);
		out.push('');
		open = true;
	}
	close();
	return out.join('\n');
}

/**
 * Rewrite every `| Signature | Description |` table in `body` into expandable
 * entries. A table with any other shape is left exactly as it is.
 */
function renderApiTables(body) {
	const lines = body.split('\n');
	const out = [];
	for (let i = 0; i < lines.length; i++) {
		const header = lines[i].match(/^\|\s*Signature\s*\|\s*Description\s*\|\s*$/);
		if (!header || !/^\|\s*-+\s*\|\s*-+\s*\|\s*$/.test(lines[i + 1] ?? '')) {
			out.push(lines[i]);
			continue;
		}
		const rows = [];
		let j = i + 2;
		for (; j < lines.length && lines[j].startsWith('|'); j++) {
			// `\|` is an escaped pipe inside a cell, not a column break.
			const cells = lines[j]
				.replace(/\\\|/g, '\u0001')
				.split('|')
				.slice(1, -1)
				.map((c) => c.trim().replace(/\u0001/g, '\\|'));
			if (cells.length < 2) break;
			rows.push(cells);
		}
		if (rows.length === 0) {
			out.push(lines[i]);
			continue;
		}
		out.push('<div class="api-list">');
		for (const [sigCell, descCell] of rows) {
			const sig = sigCell.replace(/^`|`$/g, '');
			const anchor = signatureAnchor(sig);
			const { prose, examples } = splitExamples(descCell);
			// A blank line after the `<summary>` hands the rest back to the
			// markdown parser, so the description keeps its links and the
			// example becomes a real fenced block — highlighted by Expressive
			// Code with the project's own TextMate grammar, exactly as the
			// playground and every other code block on the site is. A
			// hand-rolled tokeniser cannot stay in step with that grammar and
			// did not: it invented its own palette.
			out.push(`<details class="api-entry" id="${anchor}">`);
			out.push(`<summary><code class="api-sig">${highlightSignature(sig)}</code></summary>`);
			out.push('');
			out.push(prose);
			if (examples.length > 0) {
				out.push('');
				out.push('```saule');
				out.push(...examples);
				out.push('```');
			}
			out.push('');
			out.push('</details>');
		}
		out.push('</div>');
		i = j - 1;
	}
	return out.join('\n');
}

function write(plans, index) {
	for (const plan of plans) {
		for (const section of plan.sections) {
			const title = plainText(section.title);
			const description = firstParagraph(section.body);
			const orderList = ORDER[section.dir];
			const position = orderList?.indexOf(section.fileSlug) ?? -1;

			const frontmatter = [
				'---',
				`title: ${yamlString(title)}`,
				description ? `description: ${yamlString(description)}` : null,
				position >= 0 ? 'sidebar:' : null,
				position >= 0 ? `  order: ${position + 1}` : null,
				'---',
				'',
				`<!-- Generated from ${plan.file} by \`npm run sync-docs\`. Edit that file, not this one. -->`,
				'',
			]
				.filter((l) => l !== null)
				.join('\n');

			let body = restoreSauleFences(
				rewriteLinks(section.body, index, {
					file: plan.file,
					section: title,
				})
			);
			// Only the standard library: those are the reference pages, where
			// a reader is looking one function up rather than reading through.
			if (section.dir === 'stdlib') body = renderApiEntries(renderApiTables(body));

			const dir = join(docsRoot, section.dir);
			mkdirSync(dir, { recursive: true });
			// Plain `.md`, not `.mdx`: this content is lifted verbatim from a
			// hand-written Markdown source, so a stray `{` or `<` in future
			// prose would be parsed as JSX and fail the build. Nothing here
			// needs components.
			writeFileSync(join(dir, `${section.fileSlug}.md`), `${frontmatter}\n${body}\n`, 'utf8');
			console.log(`  → ${section.dir}/${section.fileSlug}.md  (${title})`);
		}
	}
}

// ---------------------------------------------------------------------------
// Examples
// ---------------------------------------------------------------------------

/**
 * The example projects under `examples/`, in the order they should appear.
 * Each entry supplies the prose; the code is read from the actual project so
 * a page can never show source that no longer compiles.
 *
 * `files` lists the sources to embed, in reading order. `run` is the command
 * that actually works for that project — several take arguments, so a bare
 * `saule run` would just print usage. `note` carries a prerequisite.
 *
 * Projects are left out of this list on purpose when they exist only as a
 * fixture (`native-package` has no Saule source of its own) or when the source
 * is too long to read on a web page (`json`, at ~500 lines, is linked instead).
 */
const EXAMPLES = [
	{
		dir: 'fs-info-example',
		title: 'Filesystem Info',
		blurb:
			'Inspects one path with `Os.fsInfo` and reports what it is. Shows nullable returns for paths that may not exist, and `match` over an enum with a `_` fallback.',
		run: 'saule run -- .',
		files: ['src/main.sau'],
	},
	{
		dir: 'vector-math',
		title: 'Operator Overloading',
		blurb:
			'A `Vec2` that adds, negates, compares and prints like a built-in number, plus a `Path` where `#` counts points and concatenation joins two paths. Shows how each operator is an interface whose method supplies both the behaviour and the result type.',
		run: 'saule run',
		files: ['src/vec2.sau', 'src/path.sau', 'src/main.sau'],
	},
	{
		dir: 'bitwise-flags',
		title: 'Bitwise Operators',
		// No literal `|` in this blurb: the index page renders it inside a
		// markdown table cell with backticks stripped, where a pipe would
		// start a new column.
		blurb:
			'A `Permissions` flag set that unions, intersects and flips with the bitwise operators and prints like `rwx`, plus an RGBA colour packed into one `integer` with shifts and masks. Shows the Lua 5.3 spellings (`~` is xor, because `^` is already exponentiation), the precedence that makes `bits & flag != 0` need no parentheses, and the `Op*` interfaces that put all six operators on a class.',
		run: 'saule run',
		files: ['src/permissions.sau', 'src/color.sau', 'src/main.sau'],
	},
	{
		dir: 'wrapper-types',
		title: 'Wrapper Types',
		// No literal `|` in this blurb — the index renders it inside a
		// markdown table cell with backticks stripped, where a pipe would
		// start a new column.
		blurb:
			'A `Text` built straight from a string literal and a `Settings` with full control of every read and write. Shows `Assignable<T>` target-typed construction and `OpIndex` / `OpNewIndex` as Saule\'s `__index` / `__newindex` — with nothing injected: a wrapper declares every method it exposes, calling the `String` class explicitly.',
		run: 'saule run',
		files: ['src/text.sau', 'src/settings.sau', 'src/main.sau'],
	},
	{
		dir: 'coroutines',
		title: 'Coroutines',
		// No literal `|` in this blurb — the index renders it inside a
		// markdown table cell with backticks stripped, where a pipe would
		// start a new column.
		blurb:
			'A six-part tour of coroutines: lazy stream combinators over infinite sources, a tokenizer that yields from two frames below the coroutine body, a recursive-descent parser that pulls one token at a time, and a cooperative scheduler whose tasks interleave on one thread — one of which fails without taking the others with it. Shows `Coroutine<T>` and `Yielder<T>`, where `T` is learned from, stackful yielding through ordinary function calls, laziness that is counted rather than claimed, and the dynamic two-way form.',
		run: 'saule run',
		files: ['src/stream.sau', 'src/lexer.sau', 'src/calc.sau', 'src/scheduler.sau', 'src/main.sau'],
	},
	// `json_usage` is deliberately absent: as of this writing it does not
	// typecheck (`Json.decode` returns `any?`, which the example assigns
	// straight to a `table?`). It stays linked under "Also in the repository"
	// until that is fixed. `todo-app` already covers using a dependency.
	{
		dir: 'todo-app',
		title: 'Todo App',
		blurb:
			'A complete command-line application: argument parsing with `Os.args()`, JSON persistence through the `json` library declared in `dependencies:`, `match` over subcommands, and rendering split into its own module.',
		run: 'saule run -- add "write some Saule"\nsaule run -- list',
		files: ['src/main.sau', 'src/storage.sau', 'src/render.sau'],
	},
	{
		dir: 'bf',
		title: 'Brainfuck Interpreter',
		blurb:
			'An interpreter for another language, in about 240 lines. Tables as a tape, a dispatch loop, and a program that takes its source file from `Os.args()`.',
		run: 'saule run -- -d          # embedded hello world\nsaule run -- test.bf     # or a .bf file\nsaule run -- 400quine.bf',
		files: ['src/main.sau', 'src/interpreter.sau'],
	},
	{
		dir: 'ui-blocks',
		title: 'Declarative UI',
		blurb:
			'A miniature SwiftUI-shaped toolkit drawn to the terminal: every widget is a class, constructing one draws it, and containers take their children as a trailing block (`Panel(title: "…") do … end`). Shows an immediate-mode layout engine that builds no widget tree, and why a block beats a table of children — `if` and `for` work inside one.',
		run: 'saule run',
		files: ['src/canvas.sau', 'src/widgets.sau', 'src/main.sau'],
	},
	{
		dir: 'toying',
		title: 'Graphics Window',
		blurb:
			'Opens a window and draws a rectangle that follows the mouse, using the Love2D-style Shine2D native package. Shows how Saule calls into a dynamically-loaded Rust library.',
		note:
			'This one needs the `shine` native package installed first. Shine2D lives in its own repository — clone [saule-shine](https://github.com/lauriszz123/saule-shine), run `cargo build --release`, and copy the built library into `~/.saule/native_packages/`.',
		run: 'saule run',
		files: ['src/main.sau'],
	},
];

function writeExamples() {
	const examplesRoot = join(repoRoot, 'examples');
	const outDir = join(docsRoot, 'examples');
	rmSync(outDir, { recursive: true, force: true });
	mkdirSync(outDir, { recursive: true });

	const listed = [];

	EXAMPLES.forEach((example, i) => {
		const projectDir = join(examplesRoot, example.dir);
		if (!existsSync(projectDir)) {
			console.warn(`  ! examples/${example.dir} does not exist — skipping`);
			return;
		}

		const parts = [
			'---',
			`title: ${yamlString(example.title)}`,
			`description: ${yamlString(plainText(example.blurb))}`,
			'sidebar:',
			`  order: ${i + 2}`, // 1 is reserved for the index page
			'---',
			'',
			`<!-- Generated from examples/${example.dir} by \`npm run sync-docs\`. Edit the example, not this file. -->`,
			'',
			example.blurb,
			'',
			`[Browse this example on GitHub](${repoUrl(`examples/${example.dir}`)})`,
			'',
		];

		if (example.note) {
			parts.push(':::caution[Prerequisite]', example.note, ':::', '');
		}

		parts.push(
			'## Run it',
			'',
			'```sh',
			`git clone ${repo}.git`,
			`cd ${shellPath(`saule/examples/${example.dir}`)}`,
			example.run,
			'```',
			''
		);

		const configPath = join(projectDir, 'saule.config');
		if (existsSync(configPath)) {
			parts.push('## `saule.config`', '', '```', readFileSync(configPath, 'utf8').trim(), '```', '');
		}

		for (const file of example.files) {
			const source = join(projectDir, file);
			if (!existsSync(source)) {
				console.warn(`  ! examples/${example.dir}/${file} is missing — skipping file`);
				continue;
			}
			parts.push(
				`## \`${file}\``,
				'',
				`\`\`\`saule title="${file}"`,
				readFileSync(source, 'utf8').replace(/\s+$/, ''),
				'```',
				''
			);
		}

		writeFileSync(join(outDir, `${example.dir}.md`), parts.join('\n'), 'utf8');
		console.log(`  → examples/${example.dir}.md  (${example.title})`);
		listed.push(example);
	});

	// A hand-written index that links the generated pages, so the Examples
	// section has a landing page rather than dropping you into whichever
	// example happens to sort first.
	const knownDirs = new Set(EXAMPLES.map((e) => e.dir));
	const extras = readdirSync(examplesRoot, { withFileTypes: true })
		.filter((d) => d.isDirectory() && !knownDirs.has(d.name))
		.map((d) => d.name);

	const index = [
		'---',
		'title: Overview',
		'description: "Complete Saule projects you can clone and run, from an 18-line tour to a Brainfuck interpreter."',
		'sidebar:',
		'  order: 1',
		'---',
		'',
		'<!-- Generated by `npm run sync-docs`. -->',
		'',
		'Every example here is a real project in the repository — the code on each',
		'page is read straight from it, so it always matches what you get when you',
		'clone and run.',
		'',
		'| Example | What it shows |',
		'|---|---|',
		...listed.map(
			(e) => `| [${e.title}](${withBase(`/examples/${e.dir}/`)}) | ${plainText(e.blurb)} |`
		),
		'',
	];

	if (extras.length) {
		index.push(
			'## Also in the repository',
			'',
			...extras.map((name) => `- [\`examples/${name}\`](${repoUrl(`examples/${name}`)})`),
			''
		);
	}

	writeFileSync(join(outDir, 'index.md'), index.join('\n'), 'utf8');
	console.log('  → examples/index.md  (Overview)');
}

console.log('Syncing repo Markdown into Starlight pages…');

// Generated directories are rebuilt from scratch every run, so a section
// renamed in the guide doesn't leave a stale orphan page behind.
for (const dir of ['language', 'stdlib']) {
	rmSync(join(docsRoot, dir), { recursive: true, force: true });
}

const plans = [
	planDocument({ file: 'LANGUAGE.md', dir: 'language', sourcePath: join(repoRoot, 'LANGUAGE.md') }),
	planDocument({ file: 'DOCS.md', dir: 'stdlib', sourcePath: join(repoRoot, 'DOCS.md') }),
];

const index = buildAnchorIndex(plans);
write(plans, index);
writeExamples();

console.log('Done.');
