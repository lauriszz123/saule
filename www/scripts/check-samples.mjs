#!/usr/bin/env node
/**
 * Run every hand-written Saule sample on the site through the real compiler.
 *
 * A documentation site that ships code which does not compile is worse than
 * one that ships less code, and Saule's syntax has enough small traps
 * (`-> T` return types, `=>` lambdas, free functions rather than string
 * methods) that reviewing samples by eye does not catch them. The compiler
 * is sitting right there in this repo, so it does the reviewing.
 *
 * Scope: **only** sources that are meant to be complete programs —
 *   - the playground examples (src/lib/playground-examples.ts)
 *   - fenced ```saule blocks in the pages under src/content/docs that this
 *     project authored by hand, opted in per-file via HAND_WRITTEN below.
 *
 * The generated pages are deliberately excluded: they come from README.md
 * and DOCS.md, where most snippets are illustrative fragments (a lone method
 * body, a type signature) that were never meant to compile standalone.
 *
 * Usage:  npm run check-samples
 * Requires `saule` on PATH, or a build at ../target/release/saule.
 */
import { readFileSync, writeFileSync, mkdtempSync, rmSync, existsSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { tmpdir } from 'node:os';

const here = dirname(fileURLToPath(import.meta.url));
const wwwRoot = join(here, '..');
const repoRoot = join(wwwRoot, '..');

/** Pages whose ```saule blocks are complete programs written for this site. */
const HAND_WRITTEN = [
	'src/content/docs/index.mdx',
	'src/content/docs/guides/introduction.md',
	'src/content/docs/guides/first-program.md',
];

/**
 * Snippets that are intentionally incomplete — a fragment shown to make a
 * point about syntax, or a file that only makes sense inside a project.
 * Keyed by a distinctive substring of the snippet.
 */
const ALLOW_INCOMPLETE = [
	'-- entities/Player.sau', // needs project mode to be imported
	'import Player from', // ditto, the consumer half
	'class Main', // project-mode entry point, not a standalone script
];

function findCompiler() {
	const local = join(repoRoot, 'target', 'release', 'saule');
	if (existsSync(local)) return local;
	const debug = join(repoRoot, 'target', 'debug', 'saule');
	if (existsSync(debug)) return debug;
	return 'saule'; // fall back to PATH
}

/** Pull every ```saule fenced block out of a Markdown/MDX document. */
function extractBlocks(markdown, file) {
	const blocks = [];
	const lines = markdown.split('\n');
	let current = null;
	let startLine = 0;

	lines.forEach((line, i) => {
		if (current === null) {
			// Opening fence, optionally with Expressive Code metadata.
			if (/^\s*```saule\b/.test(line)) {
				current = [];
				startLine = i + 1;
			}
		} else if (/^\s*```\s*$/.test(line)) {
			blocks.push({ file, line: startLine, source: current.join('\n') });
			current = null;
		} else {
			current.push(line);
		}
	});

	return blocks;
}

/**
 * Read the playground examples without a TypeScript build step: the file is
 * a plain data module, so stripping the type annotations from the two lines
 * that carry them is enough for Node to import it.
 */
async function playgroundExamples() {
	const file = join(wwwRoot, 'src', 'lib', 'playground-examples.ts');
	const ts = readFileSync(file, 'utf8');
	const js = ts
		.replace(/export interface Example \{[\s\S]*?\n\}\n/, '')
		.replace(/: Example\[\]/, '')
		.replace(/^\s*\/\*\*[\s\S]*?\*\/\s*$/m, '')
		.replace(/export const DEFAULT_EXAMPLE[\s\S]*$/, '');

	const tmp = join(tmpdir(), `saule-examples-${process.pid}.mjs`);
	writeFileSync(tmp, js, 'utf8');
	try {
		const mod = await import(`file://${tmp}`);
		return mod.EXAMPLES.map((e) => ({
			file: 'src/lib/playground-examples.ts',
			line: e.id,
			source: e.source,
		}));
	} finally {
		rmSync(tmp, { force: true });
	}
}

const compiler = findCompiler();
const workDir = mkdtempSync(join(tmpdir(), 'saule-check-'));

let checked = 0;
let skipped = 0;
const failures = [];

function check(sample) {
	if (ALLOW_INCOMPLETE.some((needle) => sample.source.includes(needle))) {
		skipped++;
		return;
	}

	const path = join(workDir, `sample-${checked}.sau`);
	writeFileSync(path, sample.source, 'utf8');
	checked++;

	try {
		execFileSync(compiler, ['run', path], {
			stdio: ['ignore', 'pipe', 'pipe'],
			timeout: 15_000,
			encoding: 'utf8',
		});
	} catch (err) {
		failures.push({
			...sample,
			detail: (err.stderr || err.stdout || err.message || '').trim(),
		});
	}
}

console.log(`Checking Saule samples with ${compiler}\n`);

for (const rel of HAND_WRITTEN) {
	const abs = join(wwwRoot, rel);
	if (!existsSync(abs)) {
		console.warn(`  ! ${rel} not found — skipping`);
		continue;
	}
	for (const block of extractBlocks(readFileSync(abs, 'utf8'), rel)) check(block);
}

for (const example of await playgroundExamples()) check(example);

rmSync(workDir, { recursive: true, force: true });

if (failures.length) {
	console.error(`✗ ${failures.length} of ${checked} samples failed to compile:\n`);
	for (const f of failures) {
		console.error(`── ${f.file}:${f.line}`);
		console.error(f.detail.split('\n').slice(0, 18).join('\n'));
		console.error('');
	}
	process.exit(1);
}

console.log(`✓ ${checked} samples compile and run (${skipped} fragments skipped).`);
