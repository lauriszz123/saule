#!/usr/bin/env node
// Set the VS Code extension's version in package.json and package-lock.json.
//
//   scripts/stamp-npm-version.mjs 26.8.0            # write
//   scripts/stamp-npm-version.mjs 26.8.0 --check    # verify only
//
// Called by scripts/stamp-version.sh; not usually run directly.
//
// This is JavaScript rather than another `perl -pi` line in the shell script
// for one reason: `package-lock.json` holds a `"version"` key for *every*
// dependency it pins, and a textual substitution rewrites all fifteen of them.
// Parsing the JSON means only the two fields that describe *this* package can
// be touched, and the lockfile cannot be corrupted by a stray match.
//
// npm keeps the version in two places and `npm ci` fails if either disagrees
// with package.json: the document root, and the `""` entry under `packages`
// that stands for the root package itself.

import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const [version, mode = 'write'] = process.argv.slice(2);
const check = mode === '--check';

if (!version || !/^\d+\.\d+\.\d+$/.test(version)) {
	console.error(`usage: stamp-npm-version.mjs <x.y.z> [--check]  (got ${version ?? 'nothing'})`);
	process.exit(2);
}

// Only the lockfile is handled here. `package.json` is hand-formatted — it
// keeps short arrays and objects on one line, which a JSON round-trip would
// expand into forty lines of noise — so stamp-version.sh edits its single
// `"version"` line textually instead. The lockfile is npm-generated, so
// reserialising it is lossless.
const TARGETS = [
	{
		file: 'editors/vscode/package-lock.json',
		paths: [['version'], ['packages', '', 'version']],
	},
];

let failed = false;

for (const { file, paths } of TARGETS) {
	const path = join(root, file);
	let text;
	try {
		text = readFileSync(path, 'utf8');
	} catch {
		console.error(`error: ${file} not found`);
		failed = true;
		continue;
	}

	const doc = JSON.parse(text);
	const stale = [];

	for (const keys of paths) {
		const parent = keys.slice(0, -1).reduce((node, key) => node?.[key], doc);
		const leaf = keys[keys.length - 1];
		if (parent === undefined || parent === null || !(leaf in parent)) {
			console.error(`error: ${file} has no ${keys.join('.')} to stamp`);
			failed = true;
			continue;
		}
		if (parent[leaf] !== version) {
			stale.push(`${keys.join('.') || 'version'} = ${parent[leaf]}`);
			parent[leaf] = version;
		}
	}

	if (check) {
		if (stale.length === 0) {
			console.log(`  ok      ${file} (npm version)`);
		} else {
			console.error(`  STALE   ${file} — ${stale.join(', ')}, expected ${version}`);
			failed = true;
		}
		continue;
	}

	if (stale.length === 0) {
		console.log(`  ok      ${file} (already ${version})`);
		continue;
	}

	// Reserialise in the file's own style, so the diff is the two version
	// lines and nothing else. npm's own two-space indent is a safe default,
	// but the line endings have to be read off the file being replaced —
	// these two are committed with CRLF, and normalising them to LF would
	// turn a version bump into a 500-line whole-file rewrite.
	const newline = text.includes('\r\n') ? '\r\n' : '\n';
	const serialised = JSON.stringify(doc, null, 2).replace(/\n/g, newline);
	writeFileSync(path, serialised + newline, 'utf8');
	console.log(`  stamped ${file} (npm version)`);
}

process.exit(failed ? 1 : 0);
