package com.saule.lang

import com.intellij.extapi.psi.PsiFileBase
import com.intellij.openapi.fileTypes.FileType
import com.intellij.psi.FileViewProvider

/** PSI root for a `.sau` file. */
class SauleFile(viewProvider: FileViewProvider) : PsiFileBase(viewProvider, SauleLanguage) {
    override fun getFileType(): FileType = SauleFileType
    override fun toString(): String = "Saule File"
}
