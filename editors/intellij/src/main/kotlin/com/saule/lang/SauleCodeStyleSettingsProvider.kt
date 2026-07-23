package com.saule.lang

import com.intellij.application.options.IndentOptionsEditor
import com.intellij.application.options.SmartIndentOptionsEditor
import com.intellij.lang.Language
import com.intellij.psi.codeStyle.CommonCodeStyleSettings
import com.intellij.psi.codeStyle.LanguageCodeStyleSettingsProvider

/**
 * Declares that Saule has code-style settings.
 *
 * Two things depend on this, neither obvious:
 *
 * 1. **Actions on Save ▸ Reformat code** builds its file-type list from
 *    languages that have *either* a `FormattingModelBuilder`, a
 *    `CodeStyleSettingsProvider` with a page, or a provider like this one.
 *    Without it `.sau` never appears in that list, so reformat-on-save can't
 *    be enabled for it.
 * 2. The indent settings below are what the IDE sends to `saule-lsp` as the
 *    `FormattingOptions` (`tabSize` / `insertSpaces`) of a formatting request.
 *
 * Deliberately *not* a `FormattingModelBuilder`: the formatting itself is done
 * by the language server, and registering a PSI formatter here would give the
 * platform a second, competing implementation.
 */
class SauleCodeStyleSettingsProvider : LanguageCodeStyleSettingsProvider() {

    override fun getLanguage(): Language = SauleLanguage

    override fun getIndentOptionsEditor(): IndentOptionsEditor = SmartIndentOptionsEditor()

    override fun customizeDefaults(
        commonSettings: CommonCodeStyleSettings,
        indentOptions: CommonCodeStyleSettings.IndentOptions,
    ) {
        // Match what `saule fmt` emits.
        indentOptions.INDENT_SIZE = 4
        indentOptions.CONTINUATION_INDENT_SIZE = 8
        indentOptions.TAB_SIZE = 4
        indentOptions.USE_TAB_CHARACTER = false
    }

    override fun getCodeSample(settingsType: SettingsType): String = CODE_SAMPLE

    private companion object {
        val CODE_SAMPLE = """
            import * from entities.player

            export class Warrior extends Entity
                local health: integer
                static maxHealth: integer = 200

                fn init(name: string, health: integer)
                    self.super(name)
                    self.health = health
                end

                fn takeHit(amount: integer) -> boolean
                    self.health = self.health - amount

                    for i: integer in {1, 2, 3} do
                        printf("hit %d\n", i)
                    end

                    return match self.health
                        case 0 then false
                        case hp when hp < 0 then false
                        case _ then true
                    end
                end
            end
        """.trimIndent()
    }
}
