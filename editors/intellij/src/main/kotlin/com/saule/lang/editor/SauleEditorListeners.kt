package com.saule.lang.editor

import com.intellij.openapi.Disposable
import com.intellij.openapi.components.Service

/**
 * Parent disposable for the application-wide editor listeners this plugin
 * installs ([SauleParameterInfoAutoPopup], [SauleAutoDedent]).
 *
 * They are registered on `EditorFactory`'s multicaster — which outlives any one
 * project — so each has to be torn down with the project that added it.
 */
@Service(Service.Level.PROJECT)
class SauleEditorListeners : Disposable {
    override fun dispose() = Unit
}
