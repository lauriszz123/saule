package com.saule.lang.editor

import com.intellij.codeInsight.hint.ParameterInfoControllerBase
import com.intellij.codeInsight.hint.ShowParameterInfoHandler
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.components.service
import com.intellij.openapi.diagnostic.thisLogger
import com.intellij.openapi.editor.Editor
import com.intellij.openapi.editor.EditorFactory
import com.intellij.openapi.editor.event.CaretEvent
import com.intellij.openapi.editor.event.CaretListener
import com.intellij.openapi.project.DumbService
import com.intellij.openapi.project.Project
import com.intellij.openapi.startup.ProjectActivity
import com.intellij.psi.PsiDocumentManager
import com.saule.lang.SauleLanguage

/**
 * Shows parameter info whenever the caret sits inside a call's argument
 * list — on entry, and again when you come back to edit an argument.
 *
 * Why this exists: LSP4IJ contributes the parameter-info *handler*, but
 * nothing ever asks for it. The platform only auto-invokes parameter info
 * from a language's own typing/caret hooks (`JavaTypedHandler` does it for
 * `(`), and even that fires once, at the moment the `(` is typed — move
 * away and back and the popup is gone for good. Since `saule-lsp` answers
 * `textDocument/signatureHelp` at *any* offset inside the parens, the
 * trigger is caret position, not a keystroke.
 *
 * [ShowParameterInfoHandler] is invoked directly rather than through
 * `AutoPopupController`, which is gated on the "Auto-popup parameter info"
 * setting and a delay. This is the same entry point Ctrl+P uses.
 */
class SauleParameterInfoAutoPopup : ProjectActivity {

    override suspend fun execute(project: Project) {
        val parent = project.service<SauleEditorListeners>()
        EditorFactory.getInstance().eventMulticaster.addCaretListener(
            object : CaretListener {
                override fun caretPositionChanged(event: CaretEvent) {
                    showIfInsideCall(project, event.editor)
                }
            },
            parent,
        )
    }

    companion object {

        /**
         * Request parameter info if `editor` shows a Saule file and the
         * caret is inside a `(...)` argument list. No-op otherwise, and
         * no-op while a popup is already up — the existing one tracks the
         * caret on its own.
         */
        fun showIfInsideCall(project: Project, editor: Editor) {
            if (project.isDisposed || editor.isDisposed) return
            if (editor.project != null && editor.project != project) return
            if (DumbService.isDumb(project)) return
            if (ParameterInfoControllerBase.existsWithVisibleHintForEditor(editor, false)) return

            // This listener is application-wide, so reject other languages
            // before doing anything that costs more than a map lookup.
            val documentManager = PsiDocumentManager.getInstance(project)
            val cached = documentManager.getCachedPsiFile(editor.document)
            if (cached != null && !cached.language.isKindOf(SauleLanguage)) return

            val caret = editor.caretModel.offset
            val lparen = enclosingOpenParen(editor.document.charsSequence, caret) ?: return

            documentManager.performLaterWhenAllCommitted {
                if (project.isDisposed || editor.isDisposed) return@performLaterWhenAllCommitted
                // The caret may have moved on while we waited for the commit.
                if (editor.caretModel.offset != caret) return@performLaterWhenAllCommitted
                val file = documentManager.getPsiFile(editor.document)
                    ?: return@performLaterWhenAllCommitted
                if (!file.language.isKindOf(SauleLanguage)) return@performLaterWhenAllCommitted
                if (ParameterInfoControllerBase.existsWithVisibleHintForEditor(editor, false)) {
                    return@performLaterWhenAllCommitted
                }
                thisLogger().debug("saule: requesting parameter info at offset $caret")
                ApplicationManager.getApplication().invokeLater {
                    if (project.isDisposed || editor.isDisposed) return@invokeLater
                    ShowParameterInfoHandler.invoke(
                        project,
                        editor,
                        file,
                        lparen,
                        /* highlightedElement = */ null,
                        /* requestFocus = */ false,
                    )
                }
            }
        }

        /**
         * Offset of the `(` whose argument list contains `caret`, or null
         * when the caret isn't inside one.
         *
         * A backward scan is enough: the server decides whether there is
         * actually a call there, and answers with nothing if not. Strings
         * and comments aren't excluded for the same reason — a false
         * positive costs one request that returns no signature. The scan
         * is bounded so a caret deep in a large file stays cheap.
         */
        fun enclosingOpenParen(text: CharSequence, caret: Int): Int? {
            var depth = 0
            var i = caret - 1
            val limit = maxOf(0, caret - SCAN_LIMIT)
            while (i >= limit) {
                when (text[i]) {
                    ')' -> depth++
                    '(' -> if (depth == 0) return i else depth--
                }
                i--
            }
            return null
        }

        private const val SCAN_LIMIT = 4000
    }
}
