package com.saule.lang.editor

import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.application.ModalityState
import com.intellij.openapi.command.WriteCommandAction
import com.intellij.openapi.components.service
import com.intellij.openapi.editor.Editor
import com.intellij.openapi.editor.EditorFactory
import com.intellij.openapi.editor.event.DocumentEvent
import com.intellij.openapi.editor.event.DocumentListener
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.openapi.project.Project
import com.intellij.openapi.startup.ProjectActivity
import com.saule.lang.SauleFileType
import com.saule.lang.format.SauleReindent

/**
 * Dedents a block-closing keyword — `end`, `until`, `else`, `elseif`, `catch`,
 * `case` — the moment it is finished, whichever block it closes.
 *
 * [SauleTypedHandler] does this too and is the nicer path, but it is easy to
 * miss: `TypedHandlerDelegate.charTyped` is only reached when the keystroke
 * gets as far as `TypedHandler`, and while a completion popup is up the lookup
 * takes the character first, appends it to its own prefix and returns. Since
 * `saule-lsp` auto-popups completions on plain letters, the `d` of `end` very
 * often lands there instead — which is why the keyword stays a level too deep.
 *
 * Watching the document instead catches every route text can arrive by:
 * typing, lookup prefixes, and completing the keyword from the popup. The
 * re-indent has to be scheduled rather than done in place, because a document
 * may not be modified from inside its own change notification.
 */
class SauleAutoDedent : ProjectActivity {

    override suspend fun execute(project: Project) {
        EditorFactory.getInstance().eventMulticaster.addDocumentListener(
            Listener(project),
            project.service<SauleEditorListeners>(),
        )
    }

    private class Listener(private val project: Project) : DocumentListener {

        override fun documentChanged(event: DocumentEvent) {
            if (project.isDisposed) return

            // Only a plain insertion of word characters can complete a keyword.
            // Everything else — deletions, replacements, reformatting, bulk
            // updates — is left alone.
            val document = event.document
            if (event.oldLength != 0 || event.newLength == 0) return
            if (document.isInBulkUpdate) return
            if (event.newFragment.any { !it.isLetter() }) return

            val file = FileDocumentManager.getInstance().getFile(document) ?: return
            if (file.fileType != SauleFileType) return

            val caret = event.offset + event.newLength
            if (!SauleReindent.keywordTypedAt(document.charsSequence, caret)) return

            // The caret model is itself a document listener, so where the caret
            // sits right now is not settled yet; the editor is picked once the
            // change is over.
            ApplicationManager.getApplication().invokeLater(
                { reindent(document, caret) },
                ModalityState.defaultModalityState(),
                project.disposed,
            )
        }

        private fun reindent(document: com.intellij.openapi.editor.Document, caret: Int) {
            if (project.isDisposed) return
            // Typing has moved on, or something else rewrote the line: the
            // keyword we saw is no longer what is under the caret.
            if (caret > document.textLength) return
            if (!SauleReindent.keywordTypedAt(document.charsSequence, caret)) return
            val editor = editorWithCaretAt(document, caret) ?: return

            // A no-op re-indent leaves the document untouched, and the platform
            // drops a command that changed nothing — so this does not litter
            // the undo stack when `SauleTypedHandler` already got there.
            WriteCommandAction.runWriteCommandAction(project, ADJUST_INDENT, null, {
                SauleReindent.line(project, editor, caret)
            })
        }

        private fun editorWithCaretAt(
            document: com.intellij.openapi.editor.Document,
            offset: Int,
        ): Editor? =
            EditorFactory.getInstance().getEditors(document, project).firstOrNull {
                !it.isDisposed &&
                    !it.isViewer &&
                    it.caretModel.caretCount == 1 &&
                    it.caretModel.offset == offset
            }
    }

    private companion object {
        const val ADJUST_INDENT = "Adjust Indent"
    }
}
