package com.saule.lang.editor

import com.intellij.codeInsight.editorActions.TypedHandlerDelegate
import com.intellij.openapi.editor.Editor
import com.intellij.openapi.project.Project
import com.intellij.psi.PsiFile
import com.saule.lang.SauleLanguage
import com.saule.lang.format.SauleReindent

/**
 * Two typing-time behaviours the platform can't infer for Saule:
 *
 * * Block-closing keywords dedent as they are typed. Saule closes blocks with
 *   words, not braces, so there is no `}` for the platform's brace-based
 *   auto-dedent to hook onto: typing `end` on a freshly indented line would
 *   otherwise leave it one level too deep until the next reformat.
 *
 *   This only fires when the keystroke reaches the typed handlers at all —
 *   with the completion popup open the platform feeds the character straight
 *   to the lookup's prefix instead. [SauleAutoDedent] is the backstop for
 *   that; doing it here as well keeps the common case part of the typing
 *   command, so a single undo takes the indent back with the keyword.
 *
 * * Opening a call pops up parameter info. LSP4IJ contributes the
 *   *handler* (`LSPParameterInfoHandler`, wired to this language in
 *   `plugin.xml`) but nothing ever asks for it — the platform only
 *   auto-invokes parameter info from a language's own typed handler, the way
 *   `JavaTypedHandler` does for `(`. Without this, `saule-lsp`'s signature
 *   help is reachable only through Ctrl+P.
 */
class SauleTypedHandler : TypedHandlerDelegate() {

    override fun charTyped(c: Char, project: Project, editor: Editor, file: PsiFile): Result {
        if (!file.language.isKindOf(SauleLanguage)) return Result.CONTINUE

        // Mirrors the server's `signatureHelpProvider.triggerCharacters`:
        // `(` opens a call, `,` moves to the next argument, `:` starts a
        // named-argument key. The caret listener in
        // [SauleParameterInfoAutoPopup] covers moving *into* an existing
        // call; this covers the keystroke that creates one.
        if (c == '(' || c == ',' || c == ':') {
            SauleParameterInfoAutoPopup.showIfInsideCall(project, editor)
            return Result.CONTINUE
        }

        if (!c.isLetter()) return Result.CONTINUE

        val caret = editor.caretModel.offset
        // Only when the keyword is the whole line so far: `end` alone dedents,
        // `x = end` (or a keyword mid-expression) is none of our business.
        if (!SauleReindent.keywordTypedAt(editor.document.charsSequence, caret)) return Result.CONTINUE

        SauleReindent.line(project, editor, caret)
        return Result.CONTINUE
    }
}
