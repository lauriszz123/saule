package com.saule.lang

import com.intellij.openapi.fileTypes.LanguageFileType
import javax.swing.Icon

/** Binds the `.sau` extension to the Saule language. */
object SauleFileType : LanguageFileType(SauleLanguage) {
    override fun getName(): String = "Saule File"
    override fun getDescription(): String = "Saule source file"
    override fun getDefaultExtension(): String = "sau"
    override fun getIcon(): Icon = SauleIcons.FILE
}
