package com.saule.lang.format

import com.intellij.application.options.CodeStyle
import com.intellij.openapi.editor.Editor
import com.intellij.openapi.project.Project

/**
 * Rewrites the leading whitespace of a single line to the indent
 * [SauleIndentModel] asks for.
 *
 * Shared by the Enter handler and the auto-dedent typed handler; both run
 * inside the editor's own write action, so the document is edited directly
 * rather than through a formatter.
 */
internal object SauleReindent {

    /**
     * Re-indent the line containing [offset] and keep the caret where it was
     * relative to the line's text. Returns `true` if the document changed.
     */
    fun line(project: Project, editor: Editor, offset: Int): Boolean {
        val document = editor.document
        val lineNumber = document.getLineNumber(offset)
        val lineStart = document.getLineStartOffset(lineNumber)
        val lineEnd = document.getLineEndOffset(lineNumber)
        val text = document.charsSequence

        var contentStart = lineStart
        while (contentStart < lineEnd && text[contentStart].isWhitespaceButNotNewline()) contentStart++

        val wanted = SauleIndentModel
            .indentForLine(text, lineStart, lineEnd)
            .render(CodeStyle.getIndentOptions(project, document))

        val existing = text.subSequence(lineStart, contentStart).toString()
        if (existing == wanted) return false

        val caret = editor.caretModel.offset
        document.replaceString(lineStart, contentStart, wanted)

        // The caret only needs nudging when it sat at or after the indent; a
        // caret parked inside the old whitespace lands at the new end of it.
        val shift = wanted.length - existing.length
        val newCaret = if (caret >= contentStart) caret + shift else lineStart + wanted.length
        editor.caretModel.moveToOffset(newCaret.coerceIn(lineStart, document.textLength))
        return true
    }

    /**
     * True when the word being typed just before [offset] is the whole of its
     * line and could change that line's indent — i.e. it is one of
     * [SauleIndentModel.DEDENT_KEYWORDS], or it *was* one until the character
     * that has just extended it (`end` → `endless`), which has to put the
     * indent back.
     *
     * Only the text left of the caret is looked at; whatever follows on the
     * line is [SauleIndentModel]'s business, and it already answers correctly
     * for a line whose first token turns out not to be a closer.
     */
    fun keywordTypedAt(text: CharSequence, offset: Int): Boolean {
        var lineStart = offset
        while (lineStart > 0 && text[lineStart - 1] != '\n') lineStart--

        val word = text.subSequence(lineStart, offset).trim()
        if (word.isEmpty() || word.length > MAX_KEYWORD_LENGTH + 1) return false
        if (word.any { !it.isLetter() }) return false

        val typed = word.toString()
        return typed in SauleIndentModel.DEDENT_KEYWORDS ||
            typed.dropLast(1) in SauleIndentModel.DEDENT_KEYWORDS
    }

    private val MAX_KEYWORD_LENGTH = SauleIndentModel.DEDENT_KEYWORDS.maxOf { it.length }

    /** Leading indentation is horizontal whitespace only — a newline ends the line. */
    private fun Char.isWhitespaceButNotNewline(): Boolean =
        this != '\n' && this != '\r' && isWhitespace()
}
