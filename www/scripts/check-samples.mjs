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
	// `SAULE_BIN` first, matching `run_tests.sh` and `run_examples_diff.sh`:
	// this script is now a differential check, and a differential check run
	// against a stale binary proves nothing.
	if (process.env.SAULE_BIN) return process.env.SAULE_BIN;
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
let fellBack = 0;
const failures = [];
const divergences = [];

/**
 * Run one sample under one engine, returning its combined output.
 *
 * `SAULE_ENGINE` rather than a flag, because that is the spelling that also
 * restores the fallback note - which is what makes the fallback count below
 * possible (`VM_TASKS.md`, Phase 4's "the fallback note is printed only when
 * the VM was asked for").
 */
function runUnder(engine, path) {
	try {
		const out = execFileSync(compiler, ['run', path], {
			stdio: ['ignore', 'pipe', 'pipe'],
			timeout: 15_000,
			encoding: 'utf8',
			env: { ...process.env, SAULE_ENGINE: engine },
		});
		return { ok: true, out };
	} catch (err) {
		return { ok: false, out: (err.stderr || err.stdout || err.message || '').trim() };
	}
}

/** The fallback note the VM prints when it cannot compile a construct. */
const FALLBACK = 'the bytecode compiler does not handle';

/**
 * The note is a property of the engine, not of the program, so it is stripped
 * before the two outputs are compared - otherwise every falling-back sample
 * would read as a divergence.
 */
const strip = (t) => t.split('\n').filter((l) => !l.includes(FALLBACK)).join('\n');

function check(sample) {
	if (ALLOW_INCOMPLETE.some((needle) => sample.source.includes(needle))) {
		skipped++;
		return;
	}

	const path = join(workDir, `sample-${checked}.sau`);
	writeFileSync(path, sample.source, 'utf8');
	checked++;

	// Both engines, output compared rather than exit status alone. These
	// samples are the only complete Saule programs `www/` holds, so this is
	// what closes Phase 3's "the differential harness does not cover
	// `www/`". A sample that runs but prints the wrong thing under the VM is
	// exactly the failure `SAULE_DIFF=1 ./run_tests.sh` exists to catch, and
	// it was invisible here while this script ran one engine once.
	const vm = runUnder('vm', path);
	if (!vm.ok) {
		failures.push({ ...sample, detail: vm.out });
		return;
	}
	if (vm.out.includes(FALLBACK)) fellBack++;

	const interp = runUnder('interp', path);
	if (!interp.ok || strip(vm.out) !== strip(interp.out)) {
		divergences.push({
			...sample,
			detail: `--- vm\n${strip(vm.out)}\n--- interp\n${strip(interp.out)}`,
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

if (failures.length || divergences.length) {
	for (const f of failures) {
		console.error(`✗ failed to run   ── ${f.file}:${f.line}`);
		console.error(f.detail.split('\n').slice(0, 18).join('\n'));
		console.error('');
	}
	for (const d of divergences) {
		console.error(`✗ engines disagree ── ${d.file}:${d.line}`);
		console.error(d.detail.split('\n').slice(0, 24).join('\n'));
		console.error('');
	}
	console.error(
		`${failures.length} of ${checked} samples failed to run, ` +
			`${divergences.length} diverged between engines.`,
	);
	process.exit(1);
}

console.log(
	`✓ ${checked} samples compile and run under both engines, with identical ` +
		`output (${skipped} fragments skipped, ${fellBack} fell back to the ` +
		`tree-walker).`,
);
