package com.saule.lang.lexer

import com.intellij.psi.tree.IElementType
import com.saule.lang.SauleLanguage

/** An [IElementType] that belongs to the Saule language. */
class SauleTokenType(debugName: String) : IElementType(debugName, SauleLanguage) {
    override fun toString(): String = "SauleTokenType." + super.toString()
}

/**
 * The token vocabulary produced by [SauleLexer].
 *
 * These mirror the real lexer in `crates/saule-lexer`, but are *grouped* by the
 * colour they should receive rather than one-per-keyword — the IntelliJ
 * syntax-highlighting layer only cares about categories, and the semantic
 * features (hover, go-to-def, …) come from the `saule-lsp` server instead.
 */
object SauleTokenTypes {
    // Trivia
    @JvmField val LINE_COMMENT = SauleTokenType("LINE_COMMENT")
    @JvmField val BLOCK_COMMENT = SauleTokenType("BLOCK_COMMENT")

    // Literals
    @JvmField val STRING = SauleTokenType("STRING")
    @JvmField val INTEGER = SauleTokenType("INTEGER")
    @JvmField val FLOAT = SauleTokenType("FLOAT")

    // Keyword buckets (each gets its own colour on the settings page)
    @JvmField val KW_CONTROL = SauleTokenType("KW_CONTROL")       // if, for, match, return …
    @JvmField val KW_DECL = SauleTokenType("KW_DECL")            // class, fn, local, import …
    @JvmField val KW_OPERATOR = SauleTokenType("KW_OPERATOR")    // and, or, not
    @JvmField val KW_SELF = SauleTokenType("KW_SELF")           // self, super
    @JvmField val CONSTANT = SauleTokenType("CONSTANT")         // true, false, nil
    @JvmField val PRIMITIVE_TYPE = SauleTokenType("PRIMITIVE_TYPE") // integer, string, table …

    // Identifiers
    @JvmField val IDENTIFIER = SauleTokenType("IDENTIFIER")
    @JvmField val TYPE_REF = SauleTokenType("TYPE_REF")        // PascalCase → class/interface/enum
    @JvmField val FUNCTION_CALL = SauleTokenType("FUNCTION_CALL") // ident immediately before '('

    // Operators & punctuation
    @JvmField val OPERATOR = SauleTokenType("OPERATOR")
    @JvmField val LPAREN = SauleTokenType("LPAREN")
    @JvmField val RPAREN = SauleTokenType("RPAREN")
    @JvmField val LBRACE = SauleTokenType("LBRACE")
    @JvmField val RBRACE = SauleTokenType("RBRACE")
    @JvmField val LBRACKET = SauleTokenType("LBRACKET")
    @JvmField val RBRACKET = SauleTokenType("RBRACKET")
    @JvmField val COMMA = SauleTokenType("COMMA")
    @JvmField val SEMICOLON = SauleTokenType("SEMICOLON")
    @JvmField val DOT = SauleTokenType("DOT")

    /** Keyword tables consulted by the lexer. Kept in sync with
     *  `crates/saule-lexer/src/lib.rs::ident_or_keyword`. */
    val CONTROL_KEYWORDS = setOf(
        "if", "else", "elseif", "then", "end", "for", "while", "repeat", "until",
        "do", "in", "break", "continue", "return", "throw", "try", "catch",
        "match", "case", "when",
    )
    val DECL_KEYWORDS = setOf(
        "class", "interface", "enum", "fn", "local", "static", "export",
        "import", "from", "as", "extends", "implements",
    )
    val OPERATOR_KEYWORDS = setOf("and", "or", "not")
    val SELF_KEYWORDS = setOf("self", "super")
    val CONSTANT_KEYWORDS = setOf("true", "false", "nil")
    val PRIMITIVE_TYPES = setOf(
        "integer", "float", "string", "boolean", "any", "function",
        "table", "userdata", "thread", "number",
    )
}
