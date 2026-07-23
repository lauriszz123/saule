package com.saule.lang.run

import com.intellij.execution.configurations.ConfigurationFactory
import com.intellij.execution.configurations.ConfigurationTypeBase
import com.intellij.execution.configurations.ConfigurationTypeUtil
import com.intellij.execution.configurations.RunConfiguration
import com.intellij.openapi.project.Project
import com.saule.lang.SauleIcons

/** The "Saule" entry in Run/Debug Configurations ▸ Add New Configuration. */
class SauleRunConfigurationType : ConfigurationTypeBase(
    ID,
    "Saule",
    "Run a Saule script or project",
    SauleIcons.FILE,
) {
    init {
        addFactory(SauleConfigurationFactory(this))
    }

    val factory: ConfigurationFactory
        get() = configurationFactories.first()

    companion object {
        const val ID = "SauleRunConfiguration"

        fun getInstance(): SauleRunConfigurationType =
            ConfigurationTypeUtil.findConfigurationType(SauleRunConfigurationType::class.java)
    }
}

class SauleConfigurationFactory(type: SauleRunConfigurationType) : ConfigurationFactory(type) {
    override fun getId(): String = "Saule"

    override fun createTemplateConfiguration(project: Project): RunConfiguration =
        SauleRunConfiguration(project, this, "Saule")
}
