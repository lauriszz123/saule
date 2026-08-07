package com.saule.lang.format

import com.intellij.ide.actionsOnSave.ActionOnSaveComment
import com.intellij.ide.actionsOnSave.ActionOnSaveContext
import com.intellij.ide.actionsOnSave.ActionOnSaveInfo
import com.intellij.ide.actionsOnSave.ActionOnSaveInfoProvider
import com.saule.lang.SauleSettings

/**
 * Puts [SauleFormatOnSave] in the **Settings ▸ Tools ▸ Actions on Save** table,
 * next to the platform's own on-save actions — the first place anyone looks to
 * turn this off.
 */
class SauleFormatOnSaveInfoProvider : ActionOnSaveInfoProvider() {

    override fun getActionOnSaveInfos(context: ActionOnSaveContext): Collection<ActionOnSaveInfo> =
        listOf(SauleFormatOnSaveInfo(context))

    override fun getSearchableOptions(): Collection<String> =
        listOf("Reformat Saule files", "Saule", "saule fmt")
}

/**
 * The row itself. The setting is application-wide, so the checkbox state is held
 * here until the settings dialog is applied.
 */
private class SauleFormatOnSaveInfo(context: ActionOnSaveContext) : ActionOnSaveInfo(context) {

    private var enabled: Boolean = settings().formatOnSave

    override fun getActionOnSaveName(): String = "Reformat Saule files"

    override fun getComment(): ActionOnSaveComment =
        ActionOnSaveComment.info("Formats .sau files with 'saule fmt', via the Saule language server")

    override fun isActionOnSaveEnabled(): Boolean = enabled

    override fun setActionOnSaveEnabled(enabled: Boolean) {
        this.enabled = enabled
    }

    override fun isModified(): Boolean = enabled != settings().formatOnSave

    override fun apply() {
        settings().formatOnSave = enabled
    }

    private fun settings(): SauleSettings = SauleSettings.getInstance()
}
