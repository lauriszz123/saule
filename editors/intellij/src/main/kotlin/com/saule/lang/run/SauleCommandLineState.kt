package com.saule.lang.run

import com.intellij.execution.ExecutionException
import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.execution.process.KillableColoredProcessHandler
import com.intellij.execution.process.ProcessHandler
import com.intellij.execution.process.ProcessTerminatedListener
import com.intellij.execution.runners.ExecutionEnvironment
import com.intellij.execution.configurations.CommandLineState
import com.intellij.util.execution.ParametersListUtil
import com.saule.lang.SauleToolchain
import java.io.File

/**
 * Builds and launches the `saule run …` command line, streaming stdout/stderr
 * into the Run tool window with ANSI colour support.
 */
class SauleCommandLineState(
    private val config: SauleRunConfiguration,
    environment: ExecutionEnvironment,
) : CommandLineState(environment) {

    override fun startProcess(): ProcessHandler {
        val project = environment.project

        val located = if (config.exePath.isNotBlank()) {
            SauleToolchain.Located(
                config.exePath,
                config.workingDirectory.ifBlank { project.basePath },
                found = File(config.exePath).isFile,
            )
        } else {
            SauleToolchain.locate(
                project,
                base = "saule",
                overrideEnv = "SAULE_PATH",
                overrideProp = "saule.path",
            )
        }

        if (!located.found) {
            throw ExecutionException(
                "Could not find the 'saule' executable. Set the toolchain location in " +
                    "Settings | Languages & Frameworks | Saule, set this configuration's " +
                    "\"Saule executable\" field, or add 'saule' to your PATH.",
            )
        }

        // A plain command line (NOT a PTY): the Run console is a text view, not a
        // terminal emulator, so a ConPTY would leak raw escape sequences. With a
        // plain pipe, `saule`/miette sees "no TTY" and emits clean text, while
        // KillableColoredProcessHandler still decodes any SGR colour codes.
        val cmd = GeneralCommandLine(located.command)
            .withCharset(Charsets.UTF_8)

        val workDir = config.workingDirectory.ifBlank { located.workingDir ?: project.basePath }
        workDir?.let { cmd.withWorkDirectory(it) }

        // `saule run` → project mode; `saule run <file>` → single-file mode.
        cmd.addParameter("run")
        if (config.scriptPath.isNotBlank()) {
            cmd.addParameter(config.scriptPath)
        }
        if (config.programArguments.isNotBlank()) {
            cmd.addParameters(ParametersListUtil.parse(config.programArguments))
        }

        val handler = KillableColoredProcessHandler(cmd)
        ProcessTerminatedListener.attach(handler)
        return handler
    }
}
