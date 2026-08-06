// Works out the indentation of any line of Saule source.
//
// `saule-lsp` owns *formatting*, but it is not in the loop while you type, so
// the editor needs its own answer to "how far in does this line start?" for
// pressing Enter and for the auto-dedent of `end`.
//
// This is a port of the IntelliJ plugin's `SauleIndentModel` (see
// `editors/intellij/.../format/SauleIndentModel.kt`), kept deliberately
// line-for-line comparable with it so the two editors indent identically.
// Both are in turn derived from the printer in `crates/saule-fmt/src/lib.rs`;
// keep all three in step or the editors and `saule fmt` will disagree.

/** Saule's canonical layout, shared with the indent model's fallbacks. */
export const INDENT_DEFAULT = 2;

/**
 * A block that is currently open.
 *
 * `Interface` is tracked separately because an interface body holds bare
 * method *signatures* — `fn foo()` with no `end` — so `fn` must not open a
 * block there.
 *
 * `Case` is a `match` arm: `case … then` followed by a newline indents its
 * statements, and the arm is closed by the next `case` or by the `match`'s
 * `end` rather than by one of its own.
 */
const enum Frame {
  Block,
  Interface,
  Case,
  Bracket,
}

/** Keywords that open a block closed by `end` (or, for `repeat`, `until`). */
const BLOCK_OPENERS = new Set([
  "class",
  "enum",
  "if",
  "while",
  "for",
  "repeat",
  "try",
  "match",
]);

/** Keywords that continue the enclosing block: written one level out. */
const CONTINUATIONS = new Set(["else", "elseif", "catch"]);

/** Keywords that close the enclosing block: also written one level out. */
const CLOSERS = new Set(["end", "until"]);

/**
 * Every keyword that sits one level out from the body above it.
 *
 * `case` is in here because a `match` arm with a block body is closed by the
 * next `case`, not by an `end`. Typing any of these as the whole of a line is
 * what triggers the editor's auto-dedent.
 */
export const DEDENT_KEYWORDS: ReadonlySet<string> = new Set([
  ...CLOSERS,
  ...CONTINUATIONS,
  "case",
]);

const OPEN_BRACKETS = new Set(["(", "[", "{"]);
const CLOSE_BRACKETS = new Set([")", "]", "}"]);

/**
 * Tokens after which an `fn` starts a *type* rather than a definition.
 *
 * `fn(T) -> U` written as a parameter's type, a local's annotation or a return
 * type has no body and no `end`. Treating its `fn` as a block opener leaves a
 * frame on the stack that nothing ever closes, and the damage compounds: with
 * a stray block on top, the `)` closing the enclosing parameter list no longer
 * matches a bracket frame, so that stays open too.
 *
 * Only these four contexts qualify, and each admits nothing but a type: `:`
 * ascribes one, `->` introduces a return type, `<` opens a generic argument
 * list, and `as` casts to one. Notably absent are `,` and `(` — an anonymous
 * function passed as an argument (`map(xs, fn(x) … end)`) follows those, and
 * it really is a block.
 */
const TYPE_POSITION = new Set([":", "->", "<", "as"]);

/**
 * How far one source line is indented.
 *
 * Split in two because there are two widths: `blocks` is counted in *indent*
 * units (one per enclosing `class` / `fn` / `if` / … block) and
 * `continuations` in *continuation indent* units (one per still-open bracket).
 */
export class SauleIndent {
  constructor(
    readonly blocks: number,
    readonly continuations: number,
  ) {}

  static readonly ZERO = new SauleIndent(0, 0);

  equals(other: SauleIndent): boolean {
    return (
      this.blocks === other.blocks && this.continuations === other.continuations
    );
  }

  /**
   * Render as literal indent text, honouring tabs / spaces / sizes.
   *
   * VS Code has a single `tabSize`, so one continuation is two indents — the
   * ratio the IntelliJ code style defaults to (indent 2, continuation 4) and
   * the one `saule fmt` prints.
   */
  render(tabSize: number, insertSpaces: boolean): string {
    const indentSize = tabSize > 0 ? tabSize : INDENT_DEFAULT;
    const continuationSize = indentSize * 2;

    if (insertSpaces) {
      return " ".repeat(
        this.blocks * indentSize + this.continuations * continuationSize,
      );
    }
    // Tabs to indent, spaces to align — a continuation is alignment inside an
    // unclosed bracket, so it gets spaces; block levels are real indentation
    // and get hard tabs. (IntelliJ calls this "smart tabs", and defaults it on.)
    return "\t".repeat(this.blocks) + " ".repeat(this.continuations * continuationSize);
  }
}

/**
 * The indent the line spanning `[lineStart, lineEnd)` of `text` should have.
 *
 * The line's own first token matters: `end`, `else` and friends sit one level
 * out from the body they close, so the caller gets the right answer for a line
 * that is already (partly) typed as well as for an empty one.
 */
export function indentForLine(
  text: string,
  lineStart: number,
  lineEnd: number,
): SauleIndent {
  const stack: Frame[] = [];

  let firstOnLine: string | undefined;
  let previous: string | undefined;
  for (const token of tokens(text, lineEnd)) {
    if (token.start >= lineStart) {
      firstOnLine = token.word;
      break;
    }
    apply(token.word, previous, stack);
    previous = token.word;
  }

  let blocks = 0;
  for (const frame of stack) {
    if (frame !== Frame.Bracket) blocks++;
  }
  const brackets = stack.length - blocks;
  const top = stack.length > 0 ? stack[stack.length - 1] : undefined;

  let blockDedent = 0;
  let bracketDedent = 0;
  if (firstOnLine === undefined) {
    // A blank line takes the enclosing block's indent.
  } else if (CLOSERS.has(firstOnLine)) {
    blockDedent = 1 + openCaseFrames(stack);
  } else if (CONTINUATIONS.has(firstOnLine)) {
    blockDedent = 1;
  } else if (firstOnLine === "case") {
    // A `case` only steps out when a previous arm is still open; the first arm
    // of a `match` already sits at the body level.
    if (top === Frame.Case) blockDedent = 1;
  } else if (CLOSE_BRACKETS.has(firstOnLine)) {
    if (top === Frame.Bracket) bracketDedent = 1;
  }

  return new SauleIndent(
    Math.max(0, blocks - blockDedent),
    Math.max(0, brackets - bracketDedent),
  );
}

function apply(
  word: string,
  previous: string | undefined,
  stack: Frame[],
): void {
  const top = stack.length > 0 ? stack[stack.length - 1] : undefined;
  if (word === "interface") {
    stack.push(Frame.Interface);
  } else if (word === "fn") {
    // Also covers lambdas — `fn(x) … end` is a block like any other. A `fn` in
    // type position (`f: fn(T) -> U`) opens nothing: it is a type, it has no
    // body, and no `end` will ever come for it.
    if (top !== Frame.Interface && (previous === undefined || !TYPE_POSITION.has(previous))) {
      stack.push(Frame.Block);
    }
  } else if (BLOCK_OPENERS.has(word)) {
    stack.push(Frame.Block);
  } else if (CLOSERS.has(word)) {
    closeBlock(stack);
  } else if (word === "case") {
    if (top === Frame.Case) stack.pop();
    stack.push(Frame.Case);
  } else if (OPEN_BRACKETS.has(word)) {
    stack.push(Frame.Bracket);
  } else if (CLOSE_BRACKETS.has(word)) {
    if (top === Frame.Bracket) stack.pop();
  }
}

/**
 * Pop one real block. Any `case` arms — and any brackets left open by
 * half-typed code — are discarded with it, mirroring how `end` closes a
 * `match` and all of its arms at once.
 */
function closeBlock(stack: Frame[]): void {
  while (
    stack.length > 0 &&
    stack[stack.length - 1] !== Frame.Block &&
    stack[stack.length - 1] !== Frame.Interface
  ) {
    stack.pop();
  }
  if (stack.length > 0) stack.pop();
}

/** `match` arms open above the block that `end` is about to close. */
function openCaseFrames(stack: Frame[]): number {
  let n = 0;
  for (let i = stack.length - 1; i >= 0; i--) {
    if (stack[i] === Frame.Case) {
      n++;
    } else if (stack[i] === Frame.Bracket) {
      // Half-typed code; not a level of its own here.
    } else {
      return n;
    }
  }
  return n;
}

/**
 * True when the word being typed just before `offset` is the whole of its line
 * and could change that line's indent — i.e. it is one of [DEDENT_KEYWORDS],
 * or it *was* one until the character that has just extended it (`end` →
 * `endless`), which has to put the indent back.
 *
 * Only the text left of the caret is looked at; whatever follows on the line is
 * [indentForLine]'s business, and it already answers correctly for a line whose
 * first token turns out not to be a closer.
 */
export function keywordTypedAt(text: string, offset: number): boolean {
  let lineStart = offset;
  while (lineStart > 0 && text[lineStart - 1] !== "\n") lineStart--;

  const word = text.slice(lineStart, offset).trim();
  if (word.length === 0 || word.length > MAX_KEYWORD_LENGTH + 1) return false;
  if (!/^[A-Za-z]+$/.test(word)) return false;

  return DEDENT_KEYWORDS.has(word) || DEDENT_KEYWORDS.has(word.slice(0, -1));
}

const MAX_KEYWORD_LENGTH = Math.max(
  ...[...DEDENT_KEYWORDS].map((k) => k.length),
);

// ── tokenizer ───────────────────────────────────────────────────────────────

interface Token {
  readonly word: string;
  readonly start: number;
}

/**
 * The significant tokens of `text` up to `limit`, in order.
 *
 * Whitespace and comments carry no block structure and are skipped; strings are
 * skipped whole, so `"end end end"` contributes nothing. Mirrors the token
 * rules of `crates/saule-lexer` closely enough for the stack machine above —
 * in particular multi-character operators are matched longest-first, or `->`
 * would arrive as `-` then `>` and stop marking a return type.
 */
function* tokens(text: string, limit: number): Generator<Token> {
  const end = Math.min(limit, text.length);
  let i = 0;
  while (i < end) {
    const c = text[i];

    if (isSpace(c)) {
      i++;
      continue;
    }

    // Comments: `--[[ … ]]` block, `--` line. Checked before the `-` operator.
    if (c === "-" && text[i + 1] === "-") {
      if (text[i + 2] === "[" && text[i + 3] === "[") {
        const close = text.indexOf("]]", i + 4);
        i = close < 0 ? text.length : close + 2;
      } else {
        const nl = text.indexOf("\n", i + 2);
        i = nl < 0 ? text.length : nl;
      }
      continue;
    }

    // Both quote styles, as in Lua; the opening quote is the only one that
    // closes the literal, and an unterminated one stops at the newline.
    if (c === '"' || c === "'") {
      let j = i + 1;
      while (j < text.length) {
        const ch = text[j];
        if (ch === "\\") {
          j += 2;
        } else if (ch === c) {
          j++;
          break;
        } else if (ch === "\n") {
          break;
        } else {
          j++;
        }
      }
      i = j;
      continue;
    }

    if (isIdentStart(c)) {
      let j = i + 1;
      while (j < text.length && isIdentPart(text[j])) j++;
      yield { word: text.slice(i, j), start: i };
      i = j;
      continue;
    }

    if (isDigit(c)) {
      let j = i + 1;
      while (j < text.length && (isIdentPart(text[j]) || text[j] === ".")) j++;
      yield { word: text.slice(i, j), start: i };
      i = j;
      continue;
    }

    const three = text.slice(i, i + 3);
    const two = text.slice(i, i + 2);
    if (THREE_CHAR_OPS.has(three)) {
      yield { word: three, start: i };
      i += 3;
    } else if (TWO_CHAR_OPS.has(two)) {
      yield { word: two, start: i };
      i += 2;
    } else {
      yield { word: c, start: i };
      i++;
    }
  }
}

const THREE_CHAR_OPS = new Set(["...", "..="]);
const TWO_CHAR_OPS = new Set([
  "..",
  "+=",
  "-=",
  "*=",
  "/=",
  "%=",
  "^=",
  "?.",
  "??",
  "==",
  "!=",
  "<=",
  ">=",
  "->",
  "=>",
]);

function isSpace(c: string): boolean {
  return c === " " || c === "\t" || c === "\n" || c === "\r" || c === "\f" || c === "\v";
}

function isDigit(c: string): boolean {
  return c >= "0" && c <= "9";
}

function isIdentStart(c: string): boolean {
  return c === "_" || /\p{L}/u.test(c);
}

function isIdentPart(c: string): boolean {
  return c === "_" || /[\p{L}\p{N}]/u.test(c);
}
