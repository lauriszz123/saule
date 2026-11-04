#!/usr/bin/env node
/**
 * Build the playground's completion and hover index from DOCS.md.
 *
 * The standard library's reference is already the one place every signature
 * and every description is written down, and it is verified by running each
 * example. Generating from it means the editor can never describe a function
 * the documentation does not, or spell a signature the compiler would
 * reject — and adding a function to the library puts it in the playground
 * with no second edit.
 *
 * Reads `DOCS.md` and writes `src/lib/stdlib-index.json`, which
 * `src/lib/saule-complete.ts` imports. Run by `npm run build` (and `dev`)
 * through the `prebuild` hook; the output is committed so a fresh checkout
 * has it before anything runs.
 */

import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(HERE, '../..');
const DOCS = resolve(REPO, 'DOCS.md');
const OUT = resolve(HERE, '../src/lib/stdlib-index.json');

/**
 * Split a `#### ` heading's signature into the pieces the editor needs.
 *
 * `String.sub(s: string, from: integer, to: integer?) -> string` becomes
 * the owner `String`, the name `sub`, the parameter list and the return
 * type. A prelude entry such as `println(...any) -> nil` has no owner.
 */
function parseSignature(sig) {
	const open = sig.indexOf('(');
	if (open === -1) return null;
	// `Table.insert<V>(…)` — the type parameters sit between the name and
	// the parameter list, and belong to neither.
	const head = sig.slice(0, open).replace(/<.*>$/, '');
	const dot = head.lastIndexOf('.');
	const rawOwner = dot === -1 ? null : head.slice(0, dot);
	const name = dot === -1 ? head : head.slice(dot + 1);
	if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name)) return null;
	// Instance methods are documented on a sample receiver (`f.read`,
	// `p.wait`) because that is how they are called. The index wants the
	// type, since nothing is ever typed as `f`.
	const RECEIVER = { f: 'File', p: 'Process' };
	const owner = rawOwner === null ? null : (RECEIVER[rawOwner] ?? rawOwner);
	const method = rawOwner !== null && rawOwner in RECEIVER;

	// The parameter list ends at the paren that closes the one just opened.
	let depth = 0;
	let close = -1;
	for (let i = open; i < sig.length; i++) {
		if (sig[i] === '(') depth++;
		else if (sig[i] === ')') {
			depth--;
			if (depth === 0) {
				close = i;
				break;
			}
		}
	}
	if (close === -1) return null;
	const params = sig.slice(open + 1, close);
	const arrow = sig.indexOf('->', close);
	const returns = arrow === -1 ? '' : sig.slice(arrow + 2).trim();
	return { owner, name, params, returns, method };
}

/**
 * The first paragraph of prose under a heading.
 *
 * One paragraph is what fits in a completion popup. The rest of the entry —
 * the caveats and the runnable example — is a click away on the reference
 * page, which the hover card links to.
 */
function firstParagraph(body) {
	const lines = body.split('\n');
	const out = [];
	for (const line of lines) {
		const trimmed = line.trim();
		if (trimmed.startsWith('```')) break;
		if (trimmed === '') {
			if (out.length) break;
			continue;
		}
		out.push(trimmed);
	}
	return out.join(' ');
}

/**
 * `String.sub(…)` → the anchor sync-docs.mjs gives that entry.
 *
 * Built from the heading as written, so a method documented on `f` anchors
 * at `f-read` — which is what the published page actually contains.
 */
function anchorFor(sig) {
	const head = sig.slice(0, sig.indexOf('(')).replace(/<.*>$/, '');
	return head.toLowerCase().replace(/\./g, '-');
}

/** The docs page each module's entries live on, as a site-relative slug. */
const PAGE = {
	String: 'stdlib/string',
	Bytes: 'stdlib/bytes',
	Math: 'stdlib/math',
	Table: 'stdlib/table',
	Iter: 'stdlib/iter',
	Io: 'stdlib/io-file',
	File: 'stdlib/io-file',
	Os: 'stdlib/os',
	Process: 'stdlib/os',
	Saule: 'stdlib/saule',
	Project: 'stdlib/project',
};

function build() {
	const doc = readFileSync(DOCS, 'utf8');
	const lines = doc.split('\n');

	// Every heading, so an entry can be sliced out and attributed to its
	// module.
	const heads = [];
	lines.forEach((line, i) => {
		const m = /^(#{2,4}) (.+)$/.exec(line);
		if (m) heads.push({ line: i, level: m[1].length, text: m[2] });
	});

	const entries = [];
	let module = null;
	let section = null;

	heads.forEach((h, i) => {
		if (h.level === 2) {
			module = h.text.replace(/`/g, '');
			section = null;
			return;
		}
		if (h.level === 3) {
			section = h.text.replace(/`/g, '');
			return;
		}
		const sig = h.text.replace(/`/g, '');
		const parsed = parseSignature(sig);
		if (!parsed) return;

		const end = i + 1 < heads.length ? heads[i + 1].line : lines.length;
		const body = lines.slice(h.line + 1, end).join('\n');
		const page = PAGE[parsed.owner ?? ''] ?? 'stdlib/prelude-always-in-scope';

		entries.push({
			// A method completes as its bare name: it is reached through a
			// value, and the editor has no types to know which.
			label: parsed.method || !parsed.owner ? parsed.name : `${parsed.owner}.${parsed.name}`,
			owner: parsed.owner,
			name: parsed.name,
			method: parsed.method,
			signature: sig,
			params: parsed.params,
			returns: parsed.returns,
			detail: firstParagraph(body),
			section: section ?? module,
			href: `/${page}/#${anchorFor(sig)}`,
		});
	});

	// The module names themselves complete too, so typing `Str` offers
	// `String` and `.` then offers its members. Only the static classes —
	// `File` and `Process` are types you hold a value of, never names you
	// write.
	const modules = [
		...new Set(entries.filter((e) => e.owner && !e.method).map((e) => e.owner)),
	].sort();

	return { entries, modules };
}

const index = build();
mkdirSync(dirname(OUT), { recursive: true });
writeFileSync(OUT, `${JSON.stringify(index, null, '\t')}\n`);
console.log(
	`stdlib index: ${index.entries.length} entries across ${index.modules.length} modules → ${OUT}`
);
