package com.saule.lang.run

import com.intellij.openapi.fileChooser.FileChooserDescriptorFactory
import com.intellij.openapi.options.SettingsEditor
import com.intellij.openapi.project.Project
import com.intellij.openapi.ui.TextFieldWithBrowseButton
import com.intellij.ui.components.JBTextField
import com.intellij.util.ui.FormBuilder
import javax.swing.JComponent
import javax.swing.JPanel

/** The form shown in Run/Debug Configurations for a Saule configuration. */
class SauleSettingsEditor(private val project: Project) : SettingsEditor<SauleRunConfiguration>() {

    private val scriptPath = TextFieldWithBrowseButton()
    private val workingDirectory = TextFieldWithBrowseButton()
    private val programArguments = JBTextField()
    private val exePath = TextFieldWithBrowseButton()

    private val panel: JPanel

    init {
        scriptPath.addBrowseFolderListener(
            "Saule Script",
            "Choose the .sau file to run (leave empty to run the whole project)",
            project,
            FileChooserDescriptorFactory.createSingleFileDescriptor("sau"),
        )
        workingDirectory.addBrowseFolderListener(
            "Working Directory",
            "Directory to run from (must contain saule.config for project mode)",
            project,
            FileChooserDescriptorFactory.createSingleFolderDescriptor(),
        )
        exePath.addBrowseFolderListener(
            "Saule Executable",
            "Optional: path to the 'saule' binary (auto-discovered when empty)",
            project,
            FileChooserDescriptorFactory.createSingleFileDescriptor(),
        )

        panel = FormBuilder.createFormBuilder()
            .addLabeledComponent("Script file (empty = run project):", scriptPath)
            .addLabeledComponent("Program arguments:", programArguments)
            .addLabeledComponent("Working directory:", workingDirectory)
            .addLabeledComponent("Saule executable (optional):", exePath)
            .addComponentFillVertically(JPanel(), 0)
            .panel
    }

    override fun resetEditorFrom(s: SauleRunConfiguration) {
        scriptPath.text = s.scriptPath
        workingDirectory.text = s.workingDirectory
        programArguments.text = s.programArguments
        exePath.text = s.exePath
    }

    override fun applyEditorTo(s: SauleRunConfiguration) {
        s.scriptPath = scriptPath.text.trim()
        s.workingDirectory = workingDirectory.text.trim()
        s.programArguments = programArguments.text.trim()
        s.exePath = exePath.text.trim()
    }

    override fun createEditor(): JComponent = panel
}
