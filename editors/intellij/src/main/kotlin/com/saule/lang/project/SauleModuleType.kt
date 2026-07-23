package com.saule.lang.project

import com.intellij.openapi.module.ModuleType
import com.intellij.openapi.module.ModuleTypeManager
import com.saule.lang.SauleIcons
import javax.swing.Icon

/** Registers "Saule" as a project/module kind in the New Project wizard. */
class SauleModuleType : ModuleType<SauleModuleBuilder>(ID) {

    override fun createModuleBuilder(): SauleModuleBuilder = SauleModuleBuilder()
    override fun getName(): String = "Saule"
    override fun getDescription(): String =
        "Create a Saule project — scaffolds saule.config and src/main.sau."
    override fun getNodeIcon(isOpened: Boolean): Icon = SauleIcons.FILE

    companion object {
        const val ID = "SAULE_MODULE"

        val INSTANCE: SauleModuleType
            get() = ModuleTypeManager.getInstance().findByID(ID) as SauleModuleType
    }
}
