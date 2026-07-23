package com.saule.lang

import com.intellij.lang.BracePair
import com.intellij.lang.PairedBraceMatcher
import com.intellij.psi.PsiFile
import com.intellij.psi.tree.IElementType
import com.saule.lang.lexer.SauleTokenTypes as T

/** Highlights matching `()`, `{}`, `[]` pairs and enables brace-aware editing.
 *
 *  The `.sau` block delimiters `then/do … end`, `case … end`, etc. are word
 *  keywords rather than single-character braces, so they are matched by the
 *  language server's folding/structure rather than here. */
class SauleBraceMatcher : PairedBraceMatcher {
    override fun getPairs(): Array<BracePair> = PAIRS
    override fun isPairedBracesAllowedBeforeType(lbraceType: IElementType, next: IElementType?): Boolean = true
    override fun getCodeConstructStart(file: PsiFile?, openingBraceOffset: Int): Int = openingBraceOffset

    companion object {
        private val PAIRS = arrayOf(
            BracePair(T.LPAREN, T.RPAREN, false),
            BracePair(T.LBRACE, T.RBRACE, true),
            BracePair(T.LBRACKET, T.RBRACKET, false),
        )
    }
}
