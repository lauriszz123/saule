package com.saule.lang.highlight

import com.intellij.openapi.editor.DefaultLanguageHighlighterColors as D
import com.intellij.openapi.editor.colors.TextAttributesKey
import com.intellij.openapi.editor.colors.TextAttributesKey.createTextAttributesKey as key
import com.intellij.openapi.fileTypes.SyntaxHighlighterBase
import com.intellij.psi.TokenType
import com.intellij.psi.tree.IElementType
import com.saule.lang.lexer.SauleLexer
import com.saule.lang.lexer.SauleTokenTypes as T

/** Maps Saule token types onto editor colours. Each key inherits from a
 *  sensible built-in default, so it themes correctly out of the box while
 *  still being individually overridable on the colour-settings page. */
class SauleSyntaxHighlighter : SyntaxHighlighterBase() {

    override fun getHighlightingLexer() = SauleLexer()

    override fun getTokenHighlights(tokenType: IElementType): Array<TextAttributesKey> =
        KEYS[tokenType]?.let { arrayOf(it) } ?: EMPTY

    companion object {
        val LINE_COMMENT = key("SAULE_LINE_COMMENT", D.LINE_COMMENT)
        val BLOCK_COMMENT = key("SAULE_BLOCK_COMMENT", D.BLOCK_COMMENT)
        val STRING = key("SAULE_STRING", D.STRING)
        val NUMBER = key("SAULE_NUMBER", D.NUMBER)
        val KEYWORD = key("SAULE_KEYWORD", D.KEYWORD)
        val DECL_KEYWORD = key("SAULE_DECL_KEYWORD", D.KEYWORD)
        val OPERATOR_KEYWORD = key("SAULE_OPERATOR_KEYWORD", D.KEYWORD)
        val SELF_KEYWORD = key("SAULE_SELF", D.KEYWORD)
        val CONSTANT = key("SAULE_CONSTANT", D.CONSTANT)
        val PRIMITIVE_TYPE = key("SAULE_PRIMITIVE_TYPE", D.KEYWORD)
        val TYPE_REF = key("SAULE_TYPE_REF", D.CLASS_NAME)
        val FUNCTION_CALL = key("SAULE_FUNCTION_CALL", D.FUNCTION_CALL)
        val IDENTIFIER = key("SAULE_IDENTIFIER", D.IDENTIFIER)
        val OPERATOR = key("SAULE_OPERATION_SIGN", D.OPERATION_SIGN)
        val PARENTHESES = key("SAULE_PARENTHESES", D.PARENTHESES)
        val BRACES = key("SAULE_BRACES", D.BRACES)
        val BRACKETS = key("SAULE_BRACKETS", D.BRACKETS)
        val COMMA = key("SAULE_COMMA", D.COMMA)
        val SEMICOLON = key("SAULE_SEMICOLON", D.SEMICOLON)
        val DOT = key("SAULE_DOT", D.DOT)
        val BAD_CHARACTER = key("SAULE_BAD_CHARACTER", com.intellij.openapi.editor.HighlighterColors.BAD_CHARACTER)

        private val EMPTY = emptyArray<TextAttributesKey>()

        private val KEYS: Map<IElementType, TextAttributesKey> = mapOf(
            T.LINE_COMMENT to LINE_COMMENT,
            T.BLOCK_COMMENT to BLOCK_COMMENT,
            T.STRING to STRING,
            T.INTEGER to NUMBER,
            T.FLOAT to NUMBER,
            T.KW_CONTROL to KEYWORD,
            T.KW_DECL to DECL_KEYWORD,
            T.KW_OPERATOR to OPERATOR_KEYWORD,
            T.KW_SELF to SELF_KEYWORD,
            T.CONSTANT to CONSTANT,
            T.PRIMITIVE_TYPE to PRIMITIVE_TYPE,
            T.TYPE_REF to TYPE_REF,
            T.FUNCTION_CALL to FUNCTION_CALL,
            T.IDENTIFIER to IDENTIFIER,
            T.OPERATOR to OPERATOR,
            T.LPAREN to PARENTHESES,
            T.RPAREN to PARENTHESES,
            T.LBRACE to BRACES,
            T.RBRACE to BRACES,
            T.LBRACKET to BRACKETS,
            T.RBRACKET to BRACKETS,
            T.COMMA to COMMA,
            T.SEMICOLON to SEMICOLON,
            T.DOT to DOT,
            TokenType.BAD_CHARACTER to BAD_CHARACTER,
        )
    }
}
