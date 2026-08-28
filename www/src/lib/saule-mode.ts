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
// No `function`: a function's type is its signature, written `fn(...) -> T`,
// and `fn` is already a declaration keyword.
const PRIMITIVES = new Set([
	'integer', 'float', 'string', 'boolean', 'any', 'nil', 'table',
	'userdata', 'thread',
]);

/**
 * The shape of a generic call's `<...>` window, mirroring the `#generics`
 * lookahead in `saule.tmLanguage.json`. The trailing `(` is what separates
 * `filter<integer>(nums)` from `a < b`; one level of nesting covers
 * `filter<table<integer>>(xs)`. The genuinely ambiguous `a < b, c > (d)`
 * matches here too — the real parser settles that with a speculative parse
 * (`Parser::try_eat_generic_call_args`), which a regex cannot do.
 */
const TYPE_ARG_CALL =
	/^[ \t]*<(?:[A-Za-z0-9_,?()\t ]|->)*(?:<(?:[A-Za-z0-9_,?()\t ]|->)*>(?:[A-Za-z0-9_,?()\t ]|->)*)*>[ \t]*\(/;

interface State {
	/** True while inside a `--[[ … ]]` block comment. */
	inComment: boolean;
	/**
	 * Nesting depth of the `<...>` type-argument list we're inside, 0 when
	 * outside. Tracked so the angle brackets colour as brackets rather than
	 * comparisons, and — the part that actually shows — so the `>>` closing
	 * `table<table<string>>` is two brackets instead of a right shift.
	 */
	angleDepth: number;
	/** True when the next non-space character is a `<` opening such a list. */
	expectTypeArgs: boolean;
	/**
	 * True when the token just read was the keyword `fn`, so the name that
	 * follows is defining a function rather than calling one.
	 */
	afterFn: boolean;
}

const sauleStream = StreamLanguage.define<State>({
	name: 'saule',

	startState: () => ({
		inComment: false,
		angleDepth: 0,
		expectTypeArgs: false,
		afterFn: false,
	}),

	token(stream: StringStream, state: State): string | null {
		// Type-argument lists are line-local, as in the TextMate grammar: an
		// unclosed `<` colours one line rather than the rest of the document.
		if (stream.sol()) {
			state.angleDepth = 0;
			state.expectTypeArgs = false;
		}

		// Block comments span lines, so they're the first thing to resolve.
		if (state.inComment) {
			if (stream.match(/^.*?\]\]/)) state.inComment = false;
			else stream.skipToEnd();
			return 'comment';
		}

		if (stream.eatSpace()) return null;

		// Read and clear in one go: only the token immediately after `fn` is a
		// declaration name, and every branch below counts as that next token.
		const afterFn = state.afterFn;
		state.afterFn = false;

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

		// Strings, in either quote style, with backslash escapes. Only the
		// delimiter that opened the literal closes it, matching the lexer. An
		// unterminated string colours to end of line rather than bleeding into
		// the rest of the document.
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

		// Numbers, following `Lexer::number` in `crates/saule-lexer` exactly —
		// highlighting that accepts more than the lexer does is worse than
		// none, because it makes a lex error look like valid code.
		//
		// Hex and binary first: their digits include `f`, so `0xFF_80f` has to
		// be claimed here before the float suffix below can take the `f`.
		// There is no octal form and no exponent notation, and `_` separates
		// digits only inside these two prefixes — in decimal, `1_000` is `1`
		// followed by the identifier `_000`.
		if (stream.match(/^0[xX][0-9a-fA-F_]+/) || stream.match(/^0[bB][01_]+/)) {
			return 'number';
		}
		// Floats before integers, so `1.5` is not read as `1` then `.5`. The
		// fractional part needs a digit after the dot — that is what keeps
		// `1..2` (concatenation) and `1.foo` (member access) intact — while
		// the integer part may be omitted entirely: `.5`.
		//
		// The `f`/`F` suffix forces a float, and only counts as a suffix when
		// no identifier character follows it, so `2f` is `2.0` but `2foo` is
		// `2` then `foo`.
		if (stream.match(/^(?:\d+\.\d+|\.\d+)(?:[fF](?![A-Za-z0-9_]))?/)) {
			return 'number';
		}
		if (stream.match(/^\d+(?:[fF](?![A-Za-z0-9_]))?/)) {
			return 'number';
		}

		// Identifiers and keywords.
		if (stream.match(/^[A-Za-z_][A-Za-z0-9_]*/)) {
			const word = stream.current();

			if (CONTROL.has(word)) return 'keyword';
			if (DECLARATION.has(word)) {
				state.afterFn = word === 'fn';
				return 'definitionKeyword';
			}
			if (LOGICAL.has(word)) return 'operatorKeyword';
			if (SELF.has(word)) return 'self';
			if (CONSTANTS.has(word)) return 'atom';
			if (PRIMITIVES.has(word)) {
				// `table` is the one generic head that cannot be a variable
				// being compared, so `table <` is never a less-than and the
				// list can be opened without a lookahead.
				if (word === 'table' && stream.match(/^[ \t]*</, false)) {
					state.expectTypeArgs = true;
				}
				return 'typeName';
			}

			// Same heuristic the TextMate grammar uses: PascalCase is a type.
			if (/^[A-Z]/.test(word)) return 'typeName';

			// `fn name` / `fn name<T, U>` — the name defines a function rather
			// than calling one, the distinction the TextMate grammar draws
			// between `entity.name.function` and `entity.name.function.call`.
			if (afterFn) {
				if (stream.match(TYPE_ARG_CALL, false)) state.expectTypeArgs = true;
				// Both modifiers: `definition` carries the meaning, `function`
				// keeps the function colour in themes that style calls but have
				// no rule for a bare definition.
				return 'variableName.definition.function';
			}

			// A lowercase identifier followed by `(` is a call, and so is one
			// followed by a type-argument list and *then* `(`.
			// `function` is a Lezer *modifier*, not a tag, so it has to be
			// applied to a base tag — a bare 'function' would only produce a
			// console warning and no highlighting.
			if (stream.match(/^\s*\(/, false)) return 'variableName.function';
			if (stream.match(TYPE_ARG_CALL, false)) {
				state.expectTypeArgs = true;
				return 'variableName.function';
			}

			return 'variableName';
		}

		// Angle brackets of a type-argument list, taken one at a time and
		// ahead of the operator rules — otherwise the `>>` closing a nested
		// list would be claimed as a right shift.
		if ((state.expectTypeArgs || state.angleDepth > 0) && stream.eat('<')) {
			// `++`, not `= 1`: the inner `table<` of `table<table<integer>>`
			// arrives here with the outer list already open.
			state.expectTypeArgs = false;
			state.angleDepth++;
			return 'bracket';
		}
		if (state.angleDepth > 0 && stream.eat('>')) {
			state.angleDepth--;
			return 'bracket';
		}

		// Operators, longest match first so `..` doesn't tokenize as two `.`,
		// `?.` doesn't tokenize as a bare `?`, `<<` doesn't tokenize as two
		// `<`, and the compound-assignment forms (`+=`, `..=`, `<<=`, …) stay
		// whole. `<=` / `>=` sit ahead of the shifts, which cannot match them.
		if (
			stream.match(
				/^(\.\.\.|\.\.=|<<=|>>=|==|!=|<=|>=|=>|->|\?\?|\?\.|\.\.|<<|>>|[+\-*/%^&|]=)/,
			)
		)
			return 'operator';
		if (stream.match(/^[+\-*/%^=<>#!?&|~]/)) return 'operator';
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
