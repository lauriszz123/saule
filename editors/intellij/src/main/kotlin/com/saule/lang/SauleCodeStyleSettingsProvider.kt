package com.saule.lang

import com.intellij.application.options.CodeStyleAbstractConfigurable
import com.intellij.application.options.CodeStyleAbstractPanel
import com.intellij.application.options.IndentOptionsEditor
import com.intellij.application.options.SmartIndentOptionsEditor
import com.intellij.application.options.TabbedLanguageCodeStylePanel
import com.intellij.lang.Language
import com.intellij.psi.codeStyle.CodeStyleConfigurable
import com.intellij.psi.codeStyle.CodeStyleSettings
import com.intellij.psi.codeStyle.CommonCodeStyleSettings
import com.intellij.psi.codeStyle.LanguageCodeStyleSettingsProvider

/**
 * **Editor ▸ Code Style ▸ Saule** — the one page that decides how `.sau` is
 * indented.
 *
 * Everything downstream reads from here:
 *
 * 1. **Typing.** [com.saule.lang.format.SauleIndentModel] renders the indent it
 *    computes with these options, so Enter, `Adjust Indent`, and the auto-dedent
 *    of `end` all produce tabs or spaces exactly as configured.
 * 2. **Reformat.** [com.saule.lang.lsp.SauleFormattingFeature] turns them into
 *    the `FormattingOptions` of a `textDocument/formatting` request
 *    (`insertSpaces` from *Use tab character*, `tabSize` from *Indent*, or from
 *    *Tab size* when tabs are on), and `saule-lsp` feeds those straight into
 *    `FmtOptions` — see `fmt_options` in `crates/saule-lsp/src/server/format.rs`.
 * 3. **Actions on Save ▸ Reformat code** builds its file-type list from the
 *    languages that have a code-style page; without this provider `.sau` never
 *    appears there.
 *
 * The protocol has a single `tabSize` where this page has both *Indent* and
 * *Tab size*, so (2) picks whichever of the two actually decides the width of
 * one level. Keeping them equal — as the defaults below do — means never having
 * to think about it.
 *
 * Deliberately *not* a `FormattingModelBuilder`: the formatting itself is done
 * by the language server, and registering a PSI formatter here would give the
 * platform a second, competing implementation.
 */
class SauleCodeStyleSettingsProvider : LanguageCodeStyleSettingsProvider() {

    override fun getLanguage(): Language = SauleLanguage

    /** Lets the settings preview pick the Saule file type (and so its colours). */
    override fun getFileExt(): String = "sau"

    /**
     * Creates the **Editor ▸ Code Style ▸ Saule** page.
     *
     * Overriding this is what makes the page exist at all — not a nicety.
     * `CodeStyleSchemesConfigurable` collects its children from
     * `CodeStyleSettingsProvider.EXTENSION_POINT_NAME` (a different extension
     * point) plus `LanguageCodeStyleSettingsProvider.getSettingsPagesProviders()`,
     * and the latter admits only providers that declare their own
     * `createConfigurable` — it checks the *declaring class* of the method by
     * reflection. Register `langCodeStyleSettingsProvider` and stop there and
     * `customizeDefaults` still applies, so `.sau` quietly picks up 2-space
     * indents, but there is no page to change them on: the status-bar indent
     * widget falls through to *Other File Types*, which governs a different
     * file type and so appears to do nothing.
     */
    override fun createConfigurable(
        baseSettings: CodeStyleSettings,
        modelSettings: CodeStyleSettings,
    ): CodeStyleConfigurable =
        object : CodeStyleAbstractConfigurable(baseSettings, modelSettings, SauleLanguage.displayName) {
            override fun createPanel(settings: CodeStyleSettings): CodeStyleAbstractPanel =
                object : TabbedLanguageCodeStylePanel(SauleLanguage, currentSettings, settings) {
                    // Only "Tabs and Indents". Spacing, wrapping and blank lines
                    // are decided by `saule fmt`'s printer, which takes no
                    // configuration beyond the indent — offering those tabs
                    // would be offering knobs that do nothing.
                    override fun initTabs(settings: CodeStyleSettings) {
                        addIndentOptionsTab(settings)
                    }
                }
        }

    /**
     * The full indent editor: *Use tab character*, *Smart tabs*, *Tab size*,
     * *Indent*, *Continuation indent* and *Keep indents on empty lines*.
     */
    override fun getIndentOptionsEditor(): IndentOptionsEditor = SmartIndentOptionsEditor()

    /** `saule fmt`'s soft line-width target — draws the right-margin guide. */
    override fun getRightMargin(settingsType: SettingsType): Int = RIGHT_MARGIN

    override fun customizeDefaults(
        commonSettings: CommonCodeStyleSettings,
        indentOptions: CommonCodeStyleSettings.IndentOptions,
    ) {
        // Match `FmtOptions::default()` in `saule-fmt`: 2 columns, spaces.
        // These are only the *defaults* — the page above is free to override
        // them, and tabs are a first-class choice.
        indentOptions.INDENT_SIZE = SauleIndentDefaults.INDENT
        indentOptions.CONTINUATION_INDENT_SIZE = SauleIndentDefaults.CONTINUATION_INDENT
        // Kept equal to INDENT_SIZE so switching *Use tab character* on doesn't
        // also change how wide a level is; see the class docs.
        indentOptions.TAB_SIZE = SauleIndentDefaults.INDENT
        indentOptions.USE_TAB_CHARACTER = false
        // Tabs-to-indent, spaces-to-align, for when the user does switch tabs on.
        indentOptions.SMART_TABS = true
        commonSettings.RIGHT_MARGIN = RIGHT_MARGIN
    }

    override fun getCodeSample(settingsType: SettingsType): String = CODE_SAMPLE

    private companion object {
        /** `MAX_WIDTH` in `crates/saule-fmt/src/lib.rs`. */
        const val RIGHT_MARGIN = 100

        // Written at the canonical 2-space indent so the preview matches what
        // `saule fmt` / `saule-lsp` actually produce.
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

/** Saule's canonical layout, shared with the indent model's fallbacks. */
object SauleIndentDefaults {
    const val INDENT = 2
    const val CONTINUATION_INDENT = 4
}
