package com.saule.lang.run

import com.intellij.execution.actions.ConfigurationContext
import com.intellij.execution.actions.LazyRunConfigurationProducer
import com.intellij.execution.configurations.ConfigurationFactory
import com.intellij.openapi.util.Ref
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.psi.PsiElement
import com.saule.lang.SauleFileType

/**
 * Gives every `.sau` file a green Run gutter icon and a right-click ▸ Run action.
 *
 * The created configuration is **project-aware**, matching the CLI's own rules:
 *   * file inside a directory tree containing `saule.config` ⇒ **project mode**
 *     (`saule run` from that root — runs the project's declared entry point);
 *   * standalone file ⇒ **single-file mode** (`saule run <file>`).
 */
class SauleRunConfigurationProducer : LazyRunConfigurationProducer<SauleRunConfiguration>() {

    override fun getConfigurationFactory(): ConfigurationFactory =
        SauleRunConfigurationType.getInstance().factory

    override fun setupConfigurationFromContext(
        configuration: SauleRunConfiguration,
        context: ConfigurationContext,
        sourceElement: Ref<PsiElement>,
    ): Boolean {
        val file = sauFile(context) ?: return false
        val configDir = findConfigDir(file)

        if (configDir != null) {
            configuration.scriptPath = ""
            configuration.workingDirectory = configDir.path
            configuration.name = "${configDir.name} (project)"
        } else {
            configuration.scriptPath = file.path
            configuration.workingDirectory = file.parent?.path.orEmpty()
            configuration.name = file.name
        }
        return true
    }

    override fun isConfigurationFromContext(
        configuration: SauleRunConfiguration,
        context: ConfigurationContext,
    ): Boolean {
        val file = sauFile(context) ?: return false
        val configDir = findConfigDir(file)
        return if (configDir != null) {
            configuration.scriptPath.isBlank() && configuration.workingDirectory == configDir.path
        } else {
            configuration.scriptPath == file.path
        }
    }

    private fun sauFile(context: ConfigurationContext): VirtualFile? {
        val file = context.psiLocation?.containingFile?.virtualFile ?: return null
        return if (file.fileType == SauleFileType) file else null
    }

    /** Nearest ancestor directory containing a `saule.config`, or null. */
    private fun findConfigDir(file: VirtualFile): VirtualFile? {
        var dir: VirtualFile? = file.parent
        var depth = 0
        while (dir != null && depth < 24) {
            if (dir.findChild("saule.config") != null) return dir
            dir = dir.parent
            depth++
        }
        return null
    }
}
