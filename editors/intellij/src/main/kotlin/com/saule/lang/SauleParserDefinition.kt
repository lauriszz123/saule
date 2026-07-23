package com.saule.lang

import com.intellij.lang.ASTNode
import com.intellij.lang.ParserDefinition
import com.intellij.lang.PsiBuilder
import com.intellij.lang.PsiParser
import com.intellij.lexer.Lexer
import com.intellij.openapi.project.Project
import com.intellij.psi.FileViewProvider
import com.intellij.psi.PsiElement
import com.intellij.psi.PsiFile
import com.intellij.psi.TokenType
import com.intellij.psi.tree.IElementType
import com.intellij.psi.tree.IFileElementType
import com.intellij.psi.tree.TokenSet
import com.intellij.extapi.psi.ASTWrapperPsiElement
import com.saule.lang.lexer.SauleLexer
import com.saule.lang.lexer.SauleTokenTypes as T

/**
 * Gives `.sau` files a real PSI tree.
 *
 * The tree is deliberately **flat** — every token becomes a leaf under the file
 * node — because all semantic work (diagnostics, navigation, formatting) is
 * done by `saule-lsp`, not by re-implementing the grammar here.
 *
 * Registering this is not optional, though: without a `ParserDefinition`
 * IntelliJ cannot create a language PSI file and silently falls back to a
 * plain-text one, which switches off everything keyed on the language —
 * the commenter, the brace matcher, and any formatting service (and so
 * "reformat on save").
 */
class SauleParserDefinition : ParserDefinition {

    override fun createLexer(project: Project?): Lexer = SauleLexer()

    override fun createParser(project: Project?): PsiParser = SaulePsiParser()

    override fun getFileNodeType(): IFileElementType = FILE

    override fun getCommentTokens(): TokenSet = COMMENTS

    override fun getStringLiteralElements(): TokenSet = STRINGS

    override fun getWhitespaceTokens(): TokenSet = WHITESPACE

    override fun createElement(node: ASTNode): PsiElement = ASTWrapperPsiElement(node)

    override fun createFile(viewProvider: FileViewProvider): PsiFile = SauleFile(viewProvider)

    companion object {
        val FILE = IFileElementType(SauleLanguage)

        /** Consulted by the platform for comment-aware actions and by
         *  "Reformat" when deciding what it may safely move. */
        val COMMENTS: TokenSet = TokenSet.create(T.LINE_COMMENT, T.BLOCK_COMMENT)
        val STRINGS: TokenSet = TokenSet.create(T.STRING)
        val WHITESPACE: TokenSet = TokenSet.create(TokenType.WHITE_SPACE)
    }
}

/** Wraps the whole token stream in the file node — no grammar, by design. */
private class SaulePsiParser : PsiParser {
    override fun parse(root: IElementType, builder: PsiBuilder): ASTNode {
        val mark = builder.mark()
        while (!builder.eof()) {
            builder.advanceLexer()
        }
        mark.done(root)
        return builder.treeBuilt
    }
}
