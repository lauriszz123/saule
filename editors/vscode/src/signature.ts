// Shows parameter info whenever the caret sits inside a call's argument list —
// on entry, and again when you come back to edit an argument.
//
// The IntelliJ plugin does this with a caret listener (see
// `SauleParameterInfoAutoPopup`), for a reason that applies here too: VS Code
// opens signature help when a trigger character is *typed*, and once it is
// dismissed, moving the caret back into the same call does not bring it back.
// `saule-lsp` answers `textDocument/signatureHelp` at any offset inside the
// parens, so the right trigger is caret position, not a keystroke.

import {
  Disposable,
  TextEditorSelectionChangeEvent,
  commands,
  window,
} from "vscode";

/** How far back to scan for an enclosing `(`; keeps a deep caret cheap. */
const SCAN_LIMIT = 4000;

export function registerSignatureHelpFollowsCaret(): Disposable {
  return window.onDidChangeTextEditorSelection(onSelectionChange);
}

function onSelectionChange(event: TextEditorSelectionChangeEvent): void {
  const editor = event.textEditor;
  if (editor.document.languageId !== "saule") return;
  // Only a single collapsed caret; a selection is not an editing position.
  if (event.selections.length !== 1 || !event.selections[0].isEmpty) return;

  const offset = editor.document.offsetAt(event.selections[0].active);
  if (enclosingOpenParen(editor.document.getText(), offset) === undefined) {
    return;
  }

  // Fire and forget: the server decides whether there is actually a call here
  // and answers with nothing if not.
  void commands.executeCommand("editor.action.triggerParameterHints");
}

/**
 * Offset of the `(` whose argument list contains `caret`, or undefined when the
 * caret isn't inside one.
 *
 * A backward scan is enough: the server decides whether there is actually a
 * call there. Strings and comments aren't excluded for the same reason — a
 * false positive costs one request that returns no signature.
 */
export function enclosingOpenParen(
  text: string,
  caret: number,
): number | undefined {
  let depth = 0;
  const limit = Math.max(0, caret - SCAN_LIMIT);
  for (let i = caret - 1; i >= limit; i--) {
    const c = text[i];
    if (c === ")") {
      depth++;
    } else if (c === "(") {
      if (depth === 0) return i;
      depth--;
    }
  }
  return undefined;
}
