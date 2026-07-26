package com.saule.lang.lsp

import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.openapi.project.Project
import com.redhat.devtools.lsp4ij.LanguageServerFactory
import com.redhat.devtools.lsp4ij.client.features.LSPClientFeatures
import com.redhat.devtools.lsp4ij.server.CannotStartServerException
import com.redhat.devtools.lsp4ij.server.OSProcessStreamConnectionProvider
import com.redhat.devtools.lsp4ij.server.StreamConnectionProvider
import com.saule.lang.SauleNotifications
import com.saule.lang.SauleToolchain

/**
 * Wires the `saule-lsp` process into LSP4IJ.
 *
 * `saule-lsp` is a `tower-lsp` server that speaks LSP over stdio, so we just
 * spawn the binary — no arguments, no TCP. LSP4IJ owns the JSON-RPC transport,
 * lifecycle, and the mapping of LSP features onto IntelliJ (diagnostics, hover,
 * go-to-definition, find-usages, document symbols, inlay hints, signature help,
 * and formatting — everything the server advertises in its capabilities).
 */
class SauleLanguageServerFactory : LanguageServerFactory {

    override fun createConnectionProvider(project: Project): StreamConnectionProvider {
        val located = SauleToolchain.locate(
            project,
            base = "saule-lsp",
            overrideEnv = "SAULE_LSP_PATH",
            overrideProp = "saule.lsp.path",
        )

        // Don't spawn a doomed bare-name process (the cryptic "CreateProcess
        // error=2"). Tell the user how to fix it and stop cleanly.
        if (!located.found) {
            SauleNotifications.warnMissingToolchain(project, "saule-lsp")
            throw CannotStartServerException(
                "Could not find the 'saule-lsp' executable. Set the toolchain location in " +
                    "Settings | Languages & Frameworks | Saule, or add 'saule-lsp' to your PATH.",
                null,
            )
        }

        val command = GeneralCommandLine(located.command).apply {
            located.workingDir?.let { withWorkDirectory(it) }
            withCharset(Charsets.UTF_8)
        }
        return object : OSProcessStreamConnectionProvider() {
            init {
                setCommandLine(command)
            }
        }
    }

    /**
     * Formatting requests carry the Saule code style rather than LSP4IJ's
     * editor-derived guess — see [SauleFormattingFeature].
     */
    override fun createClientFeatures(): LSPClientFeatures = SauleClientFeatures()
}
