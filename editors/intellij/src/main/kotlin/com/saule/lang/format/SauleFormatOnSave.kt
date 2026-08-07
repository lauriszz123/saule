package com.saule.lang.format

import com.intellij.formatting.service.AsyncDocumentFormattingService
import com.intellij.ide.actionsOnSave.impl.ActionsOnSaveFileDocumentManagerListener
import com.intellij.openapi.command.WriteCommandAction
import com.intellij.openapi.diagnostic.logger
import com.intellij.openapi.editor.Document
import com.intellij.openapi.project.Project
import com.intellij.psi.PsiDocumentManager
import com.intellij.psi.PsiFile
import com.intellij.psi.codeStyle.CodeStyleManager
import com.saule.lang.SauleFileType
import com.saule.lang.SauleSettings

/**
 * Runs `saule fmt` over every `.sau` file as it is saved — Ctrl+S, *Save All*,
 * IdeaVim's `:w`, or the autosave that fires when the IDE loses focus.
 *
 * On by default; switch it off in **Settings ▸ Tools ▸ Actions on Save** or in
 * **Settings ▸ Languages & Frameworks ▸ Saule**.
 *
 * ### Why this exists rather than the platform's *Reformat code* on save
 *
 * The formatting itself belongs to `saule-lsp`, which LSP4IJ fronts with an
 * [AsyncDocumentFormattingService]. Asked to format, that service normally hands
 * the request to a pooled thread and applies the resulting edits in a *later*
 * write action — by which time the save has long since written the old bytes to
 * disk. That is why ticking the platform's *Reformat code* box appears to do
 * nothing for `.sau`: it does format the file, just after the save rather than
 * before it.
 *
 * [AsyncDocumentFormattingService.FORMAT_DOCUMENT_SYNCHRONOUSLY] is the platform's
 * answer: with the key set, the service runs the request on the calling thread,
 * and when the caller already holds write access it also applies the edits inline
 * instead of deferring them. Doing both here means the document is fully formatted
 * by the time this returns — and *Actions on Save* only writes the file once every
 * registered action has run.
 */
class SauleFormatOnSave : ActionsOnSaveFileDocumentManagerListener.ActionOnSave() {

    override fun isEnabledForProject(project: Project): Boolean =
        SauleSettings.getInstance().formatOnSave

    override fun processDocuments(project: Project, documents: Array<Document>) {
        if (!SauleSettings.getInstance().formatOnSave) return

        val psiDocumentManager = PsiDocumentManager.getInstance(project)
        for (document in documents) {
            if (!document.isWritable) continue
            val psiFile = psiDocumentManager.getPsiFile(document) ?: continue
            if (psiFile.fileType != SauleFileType || !psiFile.isValid) continue
            reformat(project, document, psiFile)
        }
    }

    private fun reformat(project: Project, document: Document, psiFile: PsiFile) {
        document.putUserData(AsyncDocumentFormattingService.FORMAT_DOCUMENT_SYNCHRONOUSLY, true)
        try {
            WriteCommandAction.writeCommandAction(project, psiFile)
                .withName("Reformat Saule File on Save")
                .run<RuntimeException> {
                    // The formatter reads PSI, which must agree with the text we
                    // are about to send to the server.
                    PsiDocumentManager.getInstance(project).commitDocument(document)
                    CodeStyleManager.getInstance(project).reformatText(psiFile, listOf(psiFile.textRange))
                }
        } catch (e: Exception) {
            // A failed format must never cost the user their save.
            LOG.warn("Reformat on save failed for ${psiFile.name}", e)
        } finally {
            document.putUserData(AsyncDocumentFormattingService.FORMAT_DOCUMENT_SYNCHRONOUSLY, null)
        }
    }

    private companion object {
        val LOG = logger<SauleFormatOnSave>()
    }
}
