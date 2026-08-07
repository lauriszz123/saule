package com.saule.lang

import com.intellij.openapi.fileChooser.FileChooserDescriptorFactory
import com.intellij.openapi.options.Configurable
import com.intellij.openapi.ui.TextFieldWithBrowseButton
import com.intellij.ui.components.JBCheckBox
import com.intellij.ui.components.JBLabel
import com.intellij.util.ui.FormBuilder
import javax.swing.JComponent

/** Settings ▸ Languages & Frameworks ▸ Saule — where the toolchain lives. */
class SauleConfigurable : Configurable {

    private val toolchainDir = TextFieldWithBrowseButton()
    private val saulePath = TextFieldWithBrowseButton()
    private val sauleLspPath = TextFieldWithBrowseButton()
    private val formatOnSave = JBCheckBox("Reformat .sau files on save")

    override fun getDisplayName(): String = "Saule"

    override fun createComponent(): JComponent {
        toolchainDir.addBrowseFolderListener(
            "Saule Toolchain Directory",
            "Folder containing the 'saule' and 'saule-lsp' binaries",
            null,
            FileChooserDescriptorFactory.createSingleFolderDescriptor(),
        )
        saulePath.addBrowseFolderListener(
            "Saule Executable", "Path to the 'saule' binary (overrides the directory above)",
            null, FileChooserDescriptorFactory.createSingleFileDescriptor(),
        )
        sauleLspPath.addBrowseFolderListener(
            "Saule Language Server", "Path to the 'saule-lsp' binary (overrides the directory above)",
            null, FileChooserDescriptorFactory.createSingleFileDescriptor(),
        )

        return FormBuilder.createFormBuilder()
            .addComponent(
                JBLabel(
                    "<html>Point the plugin at your Saule toolchain. Needed for projects " +
                        "outside the Saule Cargo workspace, where the binaries can't be " +
                        "auto-discovered under <code>target/</code>.</html>",
                ),
            )
            .addLabeledComponent("Toolchain directory:", toolchainDir)
            .addSeparator()
            .addLabeledComponent("'saule' path (optional):", saulePath)
            .addLabeledComponent("'saule-lsp' path (optional):", sauleLspPath)
            .addSeparator()
            .addComponent(formatOnSave)
            .addComponent(
                JBLabel(
                    "<html><small>Runs the same formatter as <code>saule fmt</code> whenever a " +
                        "file is saved. Mirrored in Settings &#9656; Tools &#9656; Actions on Save." +
                        "</small></html>",
                ),
            )
            .addComponentFillVertically(javax.swing.JPanel(), 0)
            .panel
    }

    override fun isModified(): Boolean {
        val s = SauleSettings.getInstance()
        return toolchainDir.text.trim() != s.toolchainDir ||
            saulePath.text.trim() != s.saulePath ||
            sauleLspPath.text.trim() != s.sauleLspPath ||
            formatOnSave.isSelected != s.formatOnSave
    }

    override fun apply() {
        val s = SauleSettings.getInstance()
        s.toolchainDir = toolchainDir.text.trim()
        s.saulePath = saulePath.text.trim()
        s.sauleLspPath = sauleLspPath.text.trim()
        s.formatOnSave = formatOnSave.isSelected
    }

    override fun reset() {
        val s = SauleSettings.getInstance()
        toolchainDir.text = s.toolchainDir
        saulePath.text = s.saulePath
        sauleLspPath.text = s.sauleLspPath
        formatOnSave.isSelected = s.formatOnSave
    }
}
