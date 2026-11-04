/**
 * Completion and hover for the playground's editor.
 *
 * The data is `stdlib-index.json`, generated from DOCS.md by
 * `scripts/gen-stdlib-index.mjs` — so every signature the editor offers is
 * one the reference documents and one the examples were run against. There
 * is no second list to keep in step.
 *
 * This is deliberately *not* a language server. There is no type inference
 * here, and there is no attempt at one: a completion that guesses wrong
 * about a receiver's type is worse than a list the reader can skim. What it
 * knows is the shape of the text around the cursor.
 */

import {
	autocompletion,
	type Completion,
	type CompletionContext,
	type CompletionResult,
} from '@codemirror/autocomplete';
import { hoverTooltip, type Tooltip } from '@codemirror/view';
import { syntaxTree } from '@codemirror/language';
import type { EditorState, Extension } from '@codemirror/state';

import index from './stdlib-index.json';

interface Entry {
	label: string;
	owner: string | null;
	name: string;
	method: boolean;
	signature: string;
	params: string;
	returns: string;
	detail: string;
	section: string;
	href: string;
}

const ENTRIES = index.entries as Entry[];
const MODULES = index.modules as string[];

/** Every entry, by the name it is reached as. */
const BY_NAME = new Map<string, Entry[]>();
for (const e of ENTRIES) {
	const key = e.owner && !e.method ? `${e.owner}.${e.name}` : e.name;
	const list = BY_NAME.get(key);
	if (list) list.push(e);
	else BY_NAME.set(key, [e]);
}

/** The members of each static class, for completion after a `.`. */
const BY_OWNER = new Map<string, Entry[]>();
for (const e of ENTRIES) {
	if (!e.owner) continue;
	const list = BY_OWNER.get(e.owner);
	if (list) list.push(e);
	else BY_OWNER.set(e.owner, [e]);
}

/**
 * Keywords, in the groups the TextMate grammar uses.
 *
 * Hand-written rather than read from the grammar: a completion list wants
 * what is useful to type, and the grammar's job is what to colour. `end`
 * and `then` are in both; `self` is in one.
 */
const KEYWORDS = [
	'and',
	'as',
	'break',
	'case',
	'catch',
	'class',
	'continue',
	'do',
	'else',
	'elseif',
	'end',
	'enum',
	'export',
	'extends',
	'false',
	'finally',
	'fn',
	'for',
	'from',
	'if',
	'implements',
	'import',
	'in',
	'interface',
	'local',
	'match',
	'nil',
	'not',
	'or',
	'repeat',
	'return',
	'self',
	'static',
	'then',
	'throw',
	'true',
	'try',
	'until',
	'when',
	'while',
];

const TYPES = [
	'any',
	'boolean',
	'byte',
	'bytes',
	'float',
	'integer',
	'string',
	'table',
	'File',
	'Process',
];

/** A snippet's body, keyed by the word that expands it. */
const SNIPPETS: [string, string][] = [
	['class', 'class ${Name}\n    fn init()\n        ${}\n    end\nend'],
	['fn', 'fn ${name}(${})\n    ${}\nend'],
	['if', 'if ${cond} then\n    ${}\nend'],
	['for', 'for ${i} = 1, ${10} do\n    ${}\nend'],
	['forin', 'for ${item} in ${items} do\n    ${}\nend'],
	['while', 'while ${cond} do\n    ${}\nend'],
	['match', 'match ${value}\n    case ${pattern} then ${}\n    case _ then ${}\nend'],
	['try', 'try\n    ${}\ncatch e: string\n    println(e)\nend'],
	['interface', 'interface ${Name}\n    fn ${method}() -> nil\nend'],
	['enum', 'enum ${Name}\n    ${Variant}\nend'],
];

/** Markdown-ish prose to plain text, for a one-line completion detail. */
function plain(text: string): string {
	return text
		.replace(/\[([^\]]+)\]\([^)]*\)/g, '$1')
		.replace(/[`*]/g, '')
		.replace(/\s+/g, ' ')
		.trim();
}

/** The first sentence, which is what fits beside a completion. */
function summary(detail: string): string {
	const text = plain(detail);
	const stop = text.search(/\.(\s|$)/);
    return stop === -1 ? text : text.slice(0, stop + 1);
}

function completionFor(e: Entry, label: string): Completion {
	return {
		label,
		type: 'function',
		// The parameter list, which is the thing you actually need while
		// typing a call. The return type trails it, as in the signature.
		detail: `(${e.params})${e.returns ? ` -> ${e.returns}` : ''}`,
		info: () => infoDom(e),
		// Keep the library above keywords and above words already in the
		// document: it is the list someone opens completion *for*.
		boost: e.method ? 0 : 1,
	};
}

/** The card shown beside a highlighted completion, and inside a hover. */
function infoDom(e: Entry): HTMLElement {
	const wrap = document.createElement('div');
	wrap.className = 'cm-saule-info';

	const sig = document.createElement('div');
	sig.className = 'cm-saule-info-sig';
	sig.textContent = e.signature;
	wrap.append(sig);

	if (e.detail) {
		const body = document.createElement('p');
		body.className = 'cm-saule-info-body';
		body.textContent = plain(e.detail);
		wrap.append(body);
	}

	const foot = document.createElement('div');
	foot.className = 'cm-saule-info-foot';
	const where = document.createElement('span');
	where.textContent = e.owner ? `${e.owner} · ${e.section}` : e.section;
	foot.append(where);
	const link = document.createElement('a');
	link.href = withBase(e.href);
	link.target = '_blank';
	link.rel = 'noreferrer';
	link.textContent = 'Reference ↗';
	foot.append(link);
	wrap.append(foot);

	return wrap;
}

/** The site is served under a base path; a bare `/stdlib/…` would 404. */
function withBase(href: string): string {
	const base = import.meta.env.BASE_URL.replace(/\/$/, '');
	return `${base}${href}`;
}

/**
 * What to offer at the cursor.
 *
 * Three shapes, in the order they are tested:
 *
 *   * `String.` — the members of a static class, and nothing else, because
 *     the owner is written right there and anything else would be wrong.
 *   * `obj.` where `obj` is not a known class — every instance method the
 *     library has, since the type is unknown but the list is short and
 *     filtering by what is typed does the rest.
 *   * a bare word — modules, prelude functions, keywords, types, snippets.
 */
function source(context: CompletionContext): CompletionResult | null {
	const dotted = context.matchBefore(/[A-Za-z_][A-Za-z0-9_]*\.[A-Za-z0-9_]*/);
	if (dotted) {
		const [receiver, typed] = dotted.text.split('.');
		const from = dotted.from + receiver.length + 1;
		const members = BY_OWNER.get(receiver);
		if (members) {
			return {
				from,
				options: members.map((e) => completionFor(e, e.name)),
				validFor: /^[A-Za-z0-9_]*$/,
			};
		}
		// An unknown receiver: offer the instance methods, which are the
		// only members reached this way.
		if (typed.length > 0 || context.explicit) {
			return {
				from,
				options: ENTRIES.filter((e) => e.method).map((e) => completionFor(e, e.name)),
				validFor: /^[A-Za-z0-9_]*$/,
			};
		}
		return null;
	}

	const word = context.matchBefore(/[A-Za-z_][A-Za-z0-9_]*/);
	if (!word || (word.from === word.to && !context.explicit)) return null;
	// Inside a comment or a string, a completion list is noise.
	if (inTrivia(context.state, word.from)) return null;

	const options: Completion[] = [];
	for (const m of MODULES) {
		options.push({
			label: m,
			type: 'class',
			detail: 'standard library',
			boost: 2,
		});
	}
	for (const e of ENTRIES) {
		if (e.owner) continue;
		options.push(completionFor(e, e.name));
	}
	for (const [label, template] of SNIPPETS) {
		options.push({
			label,
			type: 'keyword',
			detail: 'snippet',
			apply: applySnippet(template),
			boost: -1,
		});
	}
	for (const k of KEYWORDS) {
		options.push({ label: k, type: 'keyword', boost: -2 });
	}
	for (const t of TYPES) {
		options.push({ label: t, type: 'type', boost: -2 });
	}
	return { from: word.from, options, validFor: /^[A-Za-z0-9_]*$/ };
}

/**
 * Insert a multi-line snippet at the current indentation.
 *
 * CodeMirror's own `snippet()` would do the tab stops too, but it inserts
 * the template's newlines verbatim — so a `class` expanded inside a method
 * lands its `end` at column 0. This re-indents each line to where the
 * expansion started and drops the placeholder markers.
 */
function applySnippet(template: string) {
	return (view: { dispatch: (spec: unknown) => void; state: EditorState }, _c: Completion, from: number, to: number) => {
		const line = view.state.doc.lineAt(from);
		const indent = /^[ \t]*/.exec(line.text)?.[0] ?? '';
		const body = template
			.replace(/\$\{([^}]*)\}/g, '$1')
			.split('\n')
			.map((l, i) => (i === 0 ? l : indent + l))
			.join('\n');
		// Land the cursor on the first blank placeholder line if there is
		// one, which is where the next thing typed belongs.
		const lines = body.split('\n');
		const blank = lines.findIndex((l, i) => i > 0 && l.trim() === '');
		const cursor =
			blank === -1
				? from + body.length
				: from + lines.slice(0, blank).join('\n').length + 1 + lines[blank].length;
		view.dispatch({
			changes: { from, to, insert: body },
			selection: { anchor: cursor },
		});
	};
}

/** Whether the offset sits inside a comment or a string literal. */
function inTrivia(state: EditorState, pos: number): boolean {
	const node = syntaxTree(state).resolveInner(pos, -1);
	for (let n: typeof node | null = node; n; n = n.parent) {
		if (/comment|string/i.test(n.name)) return true;
	}
	return false;
}

/** The word under `pos`, with its bounds. */
function wordAt(state: EditorState, pos: number): { text: string; from: number; to: number } | null {
	const line = state.doc.lineAt(pos);
	const col = pos - line.from;
	const text = line.text;
	if (col >= text.length && col > 0) return null;
	let start = col;
	let end = col;
	const isWord = (c: string) => /[A-Za-z0-9_]/.test(c);
	if (!isWord(text[col] ?? '')) return null;
	while (start > 0 && isWord(text[start - 1])) start--;
	while (end < text.length && isWord(text[end])) end++;
	// Take a `Module.` prefix with it, so hovering `sub` in `String.sub`
	// finds the right entry rather than every `sub` in the library.
	let qualified = text.slice(start, end);
	if (start > 0 && text[start - 1] === '.') {
		let o = start - 1;
		while (o > 0 && isWord(text[o - 1])) o--;
		const owner = text.slice(o, start - 1);
		if (BY_OWNER.has(owner)) qualified = `${owner}.${qualified}`;
	}
	return { text: qualified, from: line.from + start, to: line.from + end };
}

/** Documentation for the name under the pointer. */
function hover(state: EditorState, pos: number): Tooltip | null {
	const word = wordAt(state, pos);
	if (!word) return null;
	const found = BY_NAME.get(word.text);
	if (!found) return null;
	// An overload set — `Math.random` has three — shows every one.
	return {
		pos: word.from,
		end: word.to,
		above: true,
		create: () => {
			const dom = document.createElement('div');
			dom.className = 'cm-saule-hover';
			for (const e of found) dom.append(infoDom(e));
			return { dom };
		},
	};
}

/** Completion, hover docs and their styling, as one extension. */
export function sauleDocs(): Extension {
	return [
		autocompletion({
			override: [source],
			activateOnTyping: true,
			icons: false,
			maxRenderedOptions: 40,
		}),
		hoverTooltip((view, pos) => hover(view.state, pos), { hoverTime: 250 }),
	];
}
