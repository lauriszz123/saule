package com.saule.lang.highlight

import com.intellij.openapi.editor.colors.TextAttributesKey
import com.intellij.openapi.fileTypes.SyntaxHighlighter
import com.intellij.openapi.options.colors.AttributesDescriptor
import com.intellij.openapi.options.colors.ColorDescriptor
import com.intellij.openapi.options.colors.ColorSettingsPage
import com.saule.lang.SauleIcons
import javax.swing.Icon

/** Adds a "Saule" entry under Settings ▸ Editor ▸ Color Scheme, with a live
 *  preview so users can recolour each token category. */
class SauleColorSettingsPage : ColorSettingsPage {

    override fun getIcon(): Icon = SauleIcons.FILE
    override fun getHighlighter(): SyntaxHighlighter = SauleSyntaxHighlighter()
    override fun getDisplayName(): String = "Saule"
    override fun getAdditionalHighlightingTagToDescriptorMap(): MutableMap<String, TextAttributesKey>? = null
    override fun getAttributeDescriptors(): Array<AttributesDescriptor> = DESCRIPTORS
    override fun getColorDescriptors(): Array<ColorDescriptor> = ColorDescriptor.EMPTY_ARRAY

    override fun getDemoText(): String = """
        -- Saule syntax highlighting preview
        --[[ block comment
             spanning lines ]]
        import Player from "entities/player.sau"

        export class Warrior extends Entity implements Damageable
            local health: integer = 100
            static maxHealth: integer = 200

            fn init(name: string, health: integer)
                self.super(name)
                self.health = health
            end

            fn takeHit(amount: integer) -> boolean
                self.health = self.health - amount
                local alive: boolean = self.health > 0
                return alive
            end
        end

        fn main() -> nil
            local w: Warrior = Warrior("Arthur", 100)
            local msg: string? = nil
            local shown: string = msg ?? "no message"
            local total: float = float(3) / 2.0

            for i: integer in {1, 2, 3} do
                printf("hit %d\n", i)
            end

            local label: string = match w.health
                case 0 then "dead"
                case hp when hp < 50 then "wounded"
                case _ then "healthy"
            end
        end
    """.trimIndent()

    companion object {
        private val DESCRIPTORS = arrayOf(
            AttributesDescriptor("Comments//Line comment", SauleSyntaxHighlighter.LINE_COMMENT),
            AttributesDescriptor("Comments//Block comment", SauleSyntaxHighlighter.BLOCK_COMMENT),
            AttributesDescriptor("Keywords//Control", SauleSyntaxHighlighter.KEYWORD),
            AttributesDescriptor("Keywords//Declaration", SauleSyntaxHighlighter.DECL_KEYWORD),
            AttributesDescriptor("Keywords//Logical operator (and/or/not)", SauleSyntaxHighlighter.OPERATOR_KEYWORD),
            AttributesDescriptor("Keywords//self / super", SauleSyntaxHighlighter.SELF_KEYWORD),
            AttributesDescriptor("Keywords//Primitive type", SauleSyntaxHighlighter.PRIMITIVE_TYPE),
            AttributesDescriptor("Literals//String", SauleSyntaxHighlighter.STRING),
            AttributesDescriptor("Literals//Number", SauleSyntaxHighlighter.NUMBER),
            AttributesDescriptor("Literals//Constant (true/false/nil)", SauleSyntaxHighlighter.CONSTANT),
            AttributesDescriptor("Identifiers//Identifier", SauleSyntaxHighlighter.IDENTIFIER),
            AttributesDescriptor("Identifiers//Type reference", SauleSyntaxHighlighter.TYPE_REF),
            AttributesDescriptor("Identifiers//Function call", SauleSyntaxHighlighter.FUNCTION_CALL),
            AttributesDescriptor("Punctuation//Operator", SauleSyntaxHighlighter.OPERATOR),
            AttributesDescriptor("Punctuation//Parentheses", SauleSyntaxHighlighter.PARENTHESES),
            AttributesDescriptor("Punctuation//Braces", SauleSyntaxHighlighter.BRACES),
            AttributesDescriptor("Punctuation//Brackets", SauleSyntaxHighlighter.BRACKETS),
            AttributesDescriptor("Punctuation//Comma", SauleSyntaxHighlighter.COMMA),
            AttributesDescriptor("Punctuation//Semicolon", SauleSyntaxHighlighter.SEMICOLON),
            AttributesDescriptor("Punctuation//Dot", SauleSyntaxHighlighter.DOT),
            AttributesDescriptor("Bad character", SauleSyntaxHighlighter.BAD_CHARACTER),
        )
    }
}
