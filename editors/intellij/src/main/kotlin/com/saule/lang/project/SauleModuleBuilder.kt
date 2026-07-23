package com.saule.lang.project

import com.intellij.ide.util.projectWizard.ModuleBuilder
import com.intellij.ide.util.projectWizard.ModuleWizardStep
import com.intellij.ide.util.projectWizard.WizardContext
import com.intellij.openapi.Disposable
import com.intellij.openapi.module.ModuleType
import com.intellij.openapi.roots.ModifiableRootModel
import com.intellij.openapi.util.io.FileUtil
import com.intellij.openapi.vfs.LocalFileSystem
import java.io.File

/**
 * Scaffolds a Saule project when a new "Saule" module/project is created.
 *
 * We write the files directly (rather than shelling out to `saule init`) so the
 * wizard works even before the toolchain binary has been located.
 */
class SauleModuleBuilder : ModuleBuilder() {

    override fun getModuleType(): ModuleType<*> = SauleModuleType.INSTANCE

    override fun setupRootModel(rootModel: ModifiableRootModel) {
        val contentPath = contentEntryPath ?: return
        val baseDir = File(contentPath)
        baseDir.mkdirs()

        val projectName = rootModel.module.name.ifBlank { "saule_project" }
        SauleProjectScaffolder.scaffold(baseDir, projectName)

        // Register the content root and mark src/ as a source folder.
        val lfs = LocalFileSystem.getInstance()
        val rootFile = lfs.refreshAndFindFileByPath(FileUtil.toSystemIndependentName(contentPath))
        if (rootFile != null) {
            rootFile.refresh(false, true)
            val contentEntry = rootModel.addContentEntry(rootFile)
            rootFile.findChild("src")?.let { contentEntry.addSourceFolder(it, false) }
        }
    }

    // Keep the wizard minimal: the standard "project name + location" step is enough.
    override fun createWizardSteps(
        wizardContext: WizardContext,
        modulesProvider: com.intellij.openapi.roots.ui.configuration.ModulesProvider,
    ): Array<ModuleWizardStep> = ModuleWizardStep.EMPTY_ARRAY

    override fun getCustomOptionsStep(
        context: WizardContext?,
        parentDisposable: Disposable?,
    ): ModuleWizardStep? = null
}
