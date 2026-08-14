#!/usr/bin/env node
/**
 * Split the repo's long-form Markdown into Starlight pages.
 *
 * README.md (~1600 lines) and DOCS.md are the canonical prose for the
 * language and its standard library. Rather than fork them into the website
 * — where the copy would rot the first time a feature lands — this script
 * slices them on `##` boundaries into one page per section and rewrites the
 * cross-references.
 *
 * The hard part is links. Inside a single README, `[classes](#classes)` is an
 * in-page anchor; once `## Classes` becomes its own route, that anchor has to
 * become `/saule/language/classes/`. So the script first indexes *every*
 * heading in both files (h2 and h3, since some links target h3s like
 * `#escaping-any-with-as`), then rewrites each `](#...)` against that index.
 *
 * Run it with `npm run sync-docs` from `www/`. Output goes to
 * `src/content/docs/{language,stdlib,reference}` and is overwritten wholesale
 * — never hand-edit those files; edit README.md/DOCS.md instead.
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
 * links in README.md were written against. Lowercase, strip anything that
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
 * encoded. The "Also in the repository" list is built from `readdirSync`, so
 * the segments are whatever the directories are actually called — and
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
 *   `](./README.md#...)`        → the matching language page
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

	let out = body.replace(/\]\((\.\/(?:README|DOCS)\.md)?#([^)\s]+)\)/g, (match, _doc, anchor) => {
		const target = resolve(anchor);
		return target ? `](${target})` : match;
	});

	// Bare document links, with no fragment, land on each doc's first page.
	out = out
		.replace(/\]\(\.\/DOCS\.md\)/g, `](${withBase('/stdlib/prelude/')})`)
		.replace(/\]\(\.\/README\.md\)/g, `](${withBase('/guides/introduction/')})`);

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

/** Explicit sidebar ordering — teaching order, not the order of the README. */
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
		'imports-and-file-structure',
		'project-configuration',
	],
	stdlib: [
		'prelude-always-in-scope',
		'string',
		'math',
		'table',
		'io-file',
		'os',
		'saule',
		'conventions',
	],
};

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

			const body = rewriteLinks(section.body, index, {
				file: plan.file,
				section: title,
			});

			const dir = join(docsRoot, section.dir);
			mkdirSync(dir, { recursive: true });
			// Plain `.md`, not `.mdx`: this content is lifted verbatim from a
			// README, so a stray `{` or `<` in future prose would be parsed as
			// JSX and fail the build. Nothing here needs components.
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
			'Opens a window and draws a rectangle that follows the mouse, using the Love2D-style `engine` native package. Shows how Saule calls into a dynamically-loaded Rust library.',
		note:
			'This one needs the `engine` native package installed first — run the `install_mac.sh` / `install_wsl.sh` / `install_windows.ps1` script for your platform from `scripts/`.',
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
// renamed in the README doesn't leave a stale orphan page behind.
for (const dir of ['language', 'stdlib']) {
	rmSync(join(docsRoot, dir), { recursive: true, force: true });
}

const plans = [
	planDocument({ file: 'README.md', dir: 'language', sourcePath: join(repoRoot, 'README.md') }),
	planDocument({ file: 'DOCS.md', dir: 'stdlib', sourcePath: join(repoRoot, 'DOCS.md') }),
];

const index = buildAnchorIndex(plans);
write(plans, index);
writeExamples();

console.log('Done.');
