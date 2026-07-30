import { StreamLanguage, LanguageSupport, type StringStream } from '@codemirror/language';

/**
 * A CodeMirror stream tokenizer for Saule.
 *
 * The website's *rendered* code blocks are highlighted by Shiki using the VS
 * Code TextMate grammar, so they can never drift from the editor. CodeMirror
 * cannot consume a TextMate grammar without pulling in Oniguruma (a ~500 kB
 * WASM regex engine), which is not a reasonable download for a playground —
 * so the live editor gets this hand-written tokenizer instead.
 *
 * It recognises the same token classes as the TextMate grammar. If a keyword
 * is added to the language, it needs adding in both places; the lists below
 * are kept in the same order as `saule.tmLanguage.json` to make that diff
 * obvious.
 */

// keyword.control.saule
const CONTROL = new Set([
	'if', 'else', 'elseif', 'then', 'end', 'for', 'while', 'repeat', 'until',
	'do', 'in', 'break', 'continue', 'return', 'throw', 'try', 'catch',
	'match', 'case', 'when',
]);

// keyword.declaration.saule
const DECLARATION = new Set([
	'class', 'interface', 'enum', 'fn', 'local', 'static', 'export', 'import',
	'from', 'as', 'extends', 'implements',
]);

// keyword.operator.logical.saule
const LOGICAL = new Set(['and', 'or', 'not']);

// variable.language.saule
const SELF = new Set(['self', 'super']);

// constant.language.saule
const CONSTANTS = new Set(['true', 'false', 'nil']);

// support.type.primitive.saule
const PRIMITIVES = new Set([
	'integer', 'float', 'string', 'boolean', 'any', 'nil', 'function', 'table',
	'userdata', 'thread', 'number',
]);

interface State {
	/** Depth of the `--[[ … ]]` block comment we're inside, 0 when outside. */
	inComment: boolean;
}

const sauleStream = StreamLanguage.define<State>({
	name: 'saule',

	startState: () => ({ inComment: false }),

	token(stream: StringStream, state: State): string | null {
		// Block comments span lines, so they're the first thing to resolve.
		if (state.inComment) {
			if (stream.match(/^.*?\]\]/)) state.inComment = false;
			else stream.skipToEnd();
			return 'comment';
		}

		if (stream.eatSpace()) return null;

		// `--[[ … ]]` block comment, then `--` line comment. Order matters:
		// a block comment opener also matches the line-comment pattern.
		if (stream.match('--[[')) {
			if (stream.match(/^.*?\]\]/)) return 'comment';
			stream.skipToEnd();
			state.inComment = true;
			return 'comment';
		}
		if (stream.match('--')) {
			stream.skipToEnd();
			return 'comment';
		}

		// Strings, with backslash escapes. An unterminated string colours to
		// end of line rather than bleeding into the rest of the document.
		const quote = stream.peek();
		if (quote === '"' || quote === "'") {
			stream.next();
			let escaped = false;
			let ch: string | void;
			while ((ch = stream.next()) != null) {
				if (ch === quote && !escaped) break;
				escaped = !escaped && ch === '\\';
			}
			return 'string';
		}

		// Numbers: hex/binary/octal prefixes, then floats, then integers.
		if (stream.match(/^0[xX][0-9a-fA-F_]+/) || stream.match(/^0[bB][01_]+/) ||
			stream.match(/^0[oO][0-7_]+/)) {
			return 'number';
		}
		if (stream.match(/^\d[\d_]*\.\d[\d_]*([eE][+-]?\d+)?/) || stream.match(/^\d[\d_]*([eE][+-]?\d+)?/)) {
			return 'number';
		}

		// Identifiers and keywords.
		if (stream.match(/^[A-Za-z_][A-Za-z0-9_]*/)) {
			const word = stream.current();

			if (CONTROL.has(word)) return 'keyword';
			if (DECLARATION.has(word)) return 'definitionKeyword';
			if (LOGICAL.has(word)) return 'operatorKeyword';
			if (SELF.has(word)) return 'self';
			if (CONSTANTS.has(word)) return 'atom';
			if (PRIMITIVES.has(word)) return 'typeName';

			// Same heuristic the TextMate grammar uses: PascalCase is a type.
			if (/^[A-Z]/.test(word)) return 'typeName';

			// A lowercase identifier immediately followed by `(` is a call.
			// `function` is a Lezer *modifier*, not a tag, so it has to be
			// applied to a base tag — a bare 'function' would only produce a
			// console warning and no highlighting.
			if (stream.match(/^\s*\(/, false)) return 'variableName.function';

			return 'variableName';
		}

		// Operators, longest match first so `..` doesn't tokenize as two `.`
		// and `?.` doesn't tokenize as a bare `?`.
		if (stream.match(/^(\.\.\.|==|!=|<=|>=|=>|->|\?\?|\?\.|\.\.)/)) return 'operator';
		if (stream.match(/^[+\-*/%=<>#!?]/)) return 'operator';
		if (stream.match(/^[(){}[\]]/)) return 'bracket';
		if (stream.match(/^[,;:.]/)) return 'punctuation';

		stream.next();
		return null;
	},

	languageData: {
		commentTokens: { line: '--', block: { open: '--[[', close: ']]' } },
		closeBrackets: { brackets: ['(', '[', '{', '"', "'"] },
		indentOnInput: /^\s*(end|else|elseif|until)\b/,
	},
});

export function saule(): LanguageSupport {
	return new LanguageSupport(sauleStream);
}
