package com.saule.lang

import com.intellij.lang.Language

/** The Saule language singleton. Every [com.intellij.psi.tree.IElementType] and
 *  every language-scoped extension point ties back to this instance. */
object SauleLanguage : Language("Saule") {
    private fun readResolve(): Any = SauleLanguage
    override fun getDisplayName(): String = "Saule"
}
