// Rewrites the leading whitespace of a single line to the indent
// [indentForLine] asks for, as you type.
//
// `saule-lsp` formats on demand but is not in the loop per keystroke, so the
// indent of a new line is computed here. This is the VS Code counterpart of
// the IntelliJ plugin's `SauleEnterHandler` (Enter), `SauleTypedHandler` and
// `SauleAutoDedent` (block-closing keywords), all of which route through
// `SauleReindent`.
//
// VS Code has one hook for both jobs — an on-type formatting provider — so the
// three collapse into one class. It is driven by `editor.formatOnType`, which
// the extension defaults to `true` for Saule files in `package.json`.

import {
  CancellationToken,
  FormattingOptions,
  OnTypeFormattingEditProvider,
  Position,
  ProviderResult,
  Range,
  TextDocument,
  TextEdit,
} from "vscode";

import { indentForLine, keywordTypedAt } from "./indent";

/**
 * Newline plus every lowercase letter.
 *
 * Saule closes blocks with `end`, not `}`, so the brace-triggered dedent every
 * other language relies on never fires — the trigger has to be the letter that
 * finishes the keyword. Registering all of them (rather than just `d`, `e`,
 * `f`, `h`, `l`) also covers the keystroke *after* a keyword, which has to put
 * the indent back when `end` turns out to be `endless`. IntelliJ watches the
 * document for the same reason.
 */
export const TRIGGER_CHARACTERS = "abcdefghijklmnopqrstuvwxyz".split("");

export class SauleOnTypeFormatting implements OnTypeFormattingEditProvider {
  provideOnTypeFormattingEdits(
    document: TextDocument,
    position: Position,
    ch: string,
    options: FormattingOptions,
    _token: CancellationToken,
  ): ProviderResult<TextEdit[]> {
    const text = document.getText();
    const offset = document.offsetAt(position);

    // A letter only matters when the word being typed is the whole of its line
    // and is (or has just stopped being) a block-closing keyword. Everything
    // else is left alone — `x = end` is none of our business.
    if (ch !== "\n" && !keywordTypedAt(text, offset)) {
      return [];
    }

    return reindentEdits(document, position, options);
  }
}

/**
 * The edit that puts the line holding `position` at its computed indent, or
 * none when it is already there.
 */
function reindentEdits(
  document: TextDocument,
  position: Position,
  options: FormattingOptions,
): TextEdit[] {
  const line = document.lineAt(position.line);
  const lineStart = document.offsetAt(line.range.start);
  const lineEnd = document.offsetAt(line.range.end);

  const wanted = indentForLine(document.getText(), lineStart, lineEnd).render(
    options.tabSize,
    options.insertSpaces,
  );

  const existing = line.text.slice(0, leadingWhitespace(line.text));
  if (existing === wanted) return [];

  return [
    TextEdit.replace(
      new Range(
        position.line,
        0,
        position.line,
        existing.length,
      ),
      wanted,
    ),
  ];
}

function leadingWhitespace(text: string): number {
  let i = 0;
  while (i < text.length && (text[i] === " " || text[i] === "\t")) i++;
  return i;
}
