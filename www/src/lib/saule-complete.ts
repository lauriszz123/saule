/**
 * Completion and hover for the playground's editor.
 *
 * **Completion** is the language server's own engine (`saule-lsp`, compiled
 * into the wasm module and run in the analysis worker), so the playground
 * suggests what the editor plugins do: the receiver's members after a `.`,
 * the locals in scope for a bare word, ranked by what fits. Only if the
 * runtime cannot load does it fall back to the static list below.
 *
 * **Hover**, and that fallback, read `stdlib-index.json`, generated from
 * DOCS.md by `scripts/gen-stdlib-index.mjs`.
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
import { completeSaule, type SauleCompletion } from './runtime';

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

/** Set once the engine has failed to load; from then on, the static list. */
let engineDown = false;

/** UTF-8 length of `text`, which is how the engine counts offsets. */
const utf8 = new TextEncoder();

/**
 * What to offer at the cursor, from the language server's engine.
 *
 * The engine decides the context itself — member, value, type, pattern,
 * import — from a real parse, and returns its list filtered and ranked. So
 * the list is shown as given (`filter: false`) and re-asked on every
 * keystroke rather than narrowed client-side, which would re-rank it by
 * CodeMirror's fuzzy score and undo the engine's ordering.
 */
async function engineSource(context: CompletionContext): Promise<CompletionResult | null> {
	if (engineDown) return staticSource(context);

	const word = context.matchBefore(/[A-Za-z_][A-Za-z0-9_]*/);
	const from = word ? word.from : context.pos;
	const before = context.state.sliceDoc(Math.max(0, from - 1), from);
	// Open on a word being typed, right after a `.` or `:` (a member or a
	// type), or when asked for explicitly with Ctrl-Space.
	const typed = word !== null && word.from < word.to;
	if (!typed && !context.explicit && before !== '.' && before !== ':') return null;

	const doc = context.state.doc.toString();
	const offset = utf8.encode(doc.slice(0, context.pos)).length;

	let items: SauleCompletion[] | null;
	try {
		items = await completeSaule(doc, offset);
	} catch {
		engineDown = true;
		return staticSource(context);
	}
	// Superseded by a newer request, or the editor moved on meanwhile.
	if (items === null || context.aborted) return null;
	if (items.length === 0) return null;

	const options: Completion[] = items.map((c, i) => ({
		label: c.label,
		type: c.kind,
		detail: c.detail,
		info: c.doc ? () => markdownInfo(c.doc!) : undefined,
		// Ignored for ordering under `filter: false`, but kept in step with
		// the engine's rank in case CodeMirror ever consults it.
		boost: Math.max(-99, 99 - i),
	}));

	// Block snippets where a statement can start — the engine offers
	// keywords exactly there.
	if (!word || before !== '.') {
		const keywords = new Set(items.filter((c) => c.kind === 'keyword').map((c) => c.label));
		const prefix = word ? word.text : '';
		for (const [label, template] of SNIPPETS) {
			if (!keywords.has(label) || !label.startsWith(prefix)) continue;
			options.push({
				label,
				type: 'text',
				detail: 'snippet',
				apply: applySnippet(template),
			});
		}
	}

	return { from, options, filter: false };
}

/**
 * The engine's documentation, which is Markdown: fenced code becomes a
 * signature block, prose becomes paragraphs. Enough for what the docs
 * contain, without a Markdown dependency.
 */
function markdownInfo(md: string): HTMLElement {
	const wrap = document.createElement('div');
	wrap.className = 'cm-saule-info';
	// The stdlib's docs end in a link to the reference; it goes in the
	// card's footer, as on the static cards, rather than in the prose.
	// Widened by hand: TypeScript does not see the callback assign it.
	let href = null as string | null;
	md = md.replace(/\[Full reference[^\]]*\]\(([^)]+)\)/, (_m, url: string) => {
		href = url;
		return '';
	});
	const parts = md.split(/```[a-z]*\n?/);
	parts.forEach((part, i) => {
		const text = part.trim();
		if (!text) return;
		if (i % 2 === 1) {
			const sig = document.createElement('div');
			sig.className = 'cm-saule-info-sig';
			sig.textContent = text;
			wrap.append(sig);
			return;
		}
		for (const para of text.split(/\n\s*\n/)) {
			const p = document.createElement('p');
			p.className = 'cm-saule-info-body';
			p.textContent = plain(para.replace(/^#+\s*/gm, '').replace(/^---$/gm, ''));
			if (p.textContent) wrap.append(p);
		}
	});
	if (href) {
		const foot = document.createElement('div');
		foot.className = 'cm-saule-info-foot';
		foot.append(document.createElement('span'));
		const link = document.createElement('a');
		link.href = href;
		link.target = '_blank';
		link.rel = 'noreferrer';
		link.textContent = 'Reference ↗';
		foot.append(link);
		wrap.append(foot);
	}
	return wrap;
}

/**
 * The fallback: what to offer at the cursor without the engine.
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
function staticSource(context: CompletionContext): CompletionResult | null {
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
			override: [engineSource],
			activateOnTyping: true,
			icons: false,
			maxRenderedOptions: 40,
		}),
		hoverTooltip((view, pos) => hover(view.state, pos), { hoverTime: 250 }),
	];
}
