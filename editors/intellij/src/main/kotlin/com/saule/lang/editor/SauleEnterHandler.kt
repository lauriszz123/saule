package com.saule.lang.editor

import com.intellij.codeInsight.editorActions.enter.EnterHandlerDelegate
import com.intellij.codeInsight.editorActions.enter.EnterHandlerDelegateAdapter
import com.intellij.openapi.actionSystem.DataContext
import com.intellij.openapi.editor.Editor
import com.intellij.psi.PsiFile
import com.saule.lang.SauleLanguage
import com.saule.lang.format.SauleReindent

/**
 * Indents the line Enter just created.
 *
 * [com.saule.lang.format.SauleLineIndentProvider] already answers the same
 * question on the way in, but only on the platform's fast path — which is
 * skipped when another delegate (unmatched-bracket completion, Enter inside a
 * comment) rewrites the document first. Running again afterwards costs nothing
 * when the indent is already right, and is what makes the result reliable.
 */
class SauleEnterHandler : EnterHandlerDelegateAdapter() {

    override fun postProcessEnter(
        file: PsiFile,
        editor: Editor,
        dataContext: DataContext,
    ): EnterHandlerDelegate.Result {
        if (!file.language.isKindOf(SauleLanguage)) return EnterHandlerDelegate.Result.Continue

        val document = editor.document
        val caret = editor.caretModel.offset
        if (caret > document.textLength) return EnterHandlerDelegate.Result.Continue

        // Only touch a caret still sitting in the fresh line's leading
        // whitespace. Anything else means a delegate has moved on to real text
        // and re-indenting would fight it.
        val lineStart = document.getLineStartOffset(document.getLineNumber(caret))
        val beforeCaret = document.charsSequence.subSequence(lineStart, caret)
        if (beforeCaret.any { !it.isWhitespace() }) return EnterHandlerDelegate.Result.Continue

        val changed = SauleReindent.line(file.project, editor, caret)
        return if (changed) EnterHandlerDelegate.Result.Stop else EnterHandlerDelegate.Result.Continue
    }
}
