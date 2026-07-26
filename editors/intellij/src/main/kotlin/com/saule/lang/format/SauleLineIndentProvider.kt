package com.saule.lang.format

import com.intellij.application.options.CodeStyle
import com.intellij.lang.Language
import com.intellij.openapi.editor.Editor
import com.intellij.openapi.project.Project
import com.intellij.psi.codeStyle.lineIndent.LineIndentProvider
import com.saule.lang.SauleLanguage

/**
 * Supplies the indent for the line Enter is about to create.
 *
 * This is the platform's "fast path": it runs on the document alone, before any
 * PSI commit, which is why the editor can indent while you type without waiting
 * on `saule-lsp`. Without a provider here the platform falls back to a
 * formatter-based one, and Saule has no `FormattingModelBuilder` (formatting is
 * the language server's job), so nothing indented at all.
 */
class SauleLineIndentProvider : LineIndentProvider {

    override fun isSuitableFor(language: Language?): Boolean =
        language != null && language.isKindOf(SauleLanguage)

    override fun getLineIndent(
        project: Project,
        editor: Editor,
        language: Language?,
        offset: Int,
    ): String? {
        if (!isSuitableFor(language)) return null
        if (offset < 0) return null

        val document = editor.document
        val current = document.charsSequence
        if (offset > current.length) return null

        // We are asked *before* the newline is inserted, so answer the question
        // for the document as it will be a moment from now.
        val text = StringBuilder(current.length + 1).append(current).insert(offset, '\n')
        val lineStart = offset + 1
        var lineEnd = lineStart
        while (lineEnd < text.length && text[lineEnd] != '\n') lineEnd++

        return SauleIndentModel
            .indentForLine(text, lineStart, lineEnd)
            .render(CodeStyle.getIndentOptions(project, document))
    }
}
