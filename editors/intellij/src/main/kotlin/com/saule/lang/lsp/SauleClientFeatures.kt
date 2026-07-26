package com.saule.lang.lsp

import com.intellij.application.options.CodeStyle
import com.intellij.openapi.application.ReadAction
import com.intellij.openapi.editor.Editor
import com.intellij.psi.PsiFile
import com.intellij.psi.codeStyle.CommonCodeStyleSettings
import com.redhat.devtools.lsp4ij.client.features.LSPClientFeatures
import com.redhat.devtools.lsp4ij.client.features.LSPFormattingFeature
import com.saule.lang.SauleIndentDefaults

/**
 * Per-server LSP4IJ overrides. Currently only formatting — see
 * [SauleFormattingFeature].
 */
class SauleClientFeatures : LSPClientFeatures() {
    init {
        setFormattingFeature(SauleFormattingFeature())
    }
}

/**
 * Sends **Editor ▸ Code Style ▸ Saule** to `saule-lsp` on every
 * `textDocument/formatting` request.
 *
 * LSP4IJ's default takes `tabSize` / `insertSpaces` from the *editor's* settings
 * ([com.redhat.devtools.lsp4ij.LSPIJUtils.getTabSize]), which has two problems:
 *
 * 1. With no open editor for the file — Reformat from the Project view, or
 *    *Actions on Save ▸ Reformat code* on a file the IDE closed — it sends no
 *    options at all, and the server then sees LSP4J's zero-value defaults
 *    (`tabSize` 0, `insertSpaces` false).
 * 2. The editor's tab size is `TAB_SIZE`, never *Indent*, so a page set to
 *    indent 4 / tab size 2 formatted to 2.
 *
 * Reading the code style straight off the [PsiFile] fixes both, and makes
 * Reformat agree with what [com.saule.lang.format.SauleIndentModel] does while
 * typing — that path reads the same options.
 */
class SauleFormattingFeature : LSPFormattingFeature() {

    /**
     * Columns per indent level.
     *
     * `tabSize` is the only width the protocol has, and `saule fmt` uses it as
     * the width of one indent level. With spaces that is *Indent*; with hard
     * tabs the printer writes one `\t` per level and uses the number only for
     * column arithmetic, so *Tab size* — how wide that `\t` renders — is the
     * right answer.
     */
    override fun getTabSize(file: PsiFile, editor: Editor?): Int {
        val options = indentOptions(file)
        val size = if (options.USE_TAB_CHARACTER) options.TAB_SIZE else options.INDENT_SIZE
        return size.takeIf { it > 0 } ?: SauleIndentDefaults.INDENT
    }

    override fun getInsertSpaces(file: PsiFile, editor: Editor?): Boolean =
        !indentOptions(file).USE_TAB_CHARACTER

    /**
     * The Saule indent options for [file].
     *
     * Wrapped in a read action because the formatting request is built on the
     * pooled thread that `AsyncDocumentFormattingService` runs the task on, and
     * resolving the options walks PSI. [ReadAction.compute] is re-entrant, so
     * this is also correct on the rare EDT call.
     */
    private fun indentOptions(file: PsiFile): CommonCodeStyleSettings.IndentOptions =
        ReadAction.compute<CommonCodeStyleSettings.IndentOptions, RuntimeException> {
            CodeStyle.getIndentOptions(file)
        }
}
