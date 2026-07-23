package com.saule.lang.run

import com.intellij.execution.Executor
import com.intellij.execution.configurations.ConfigurationFactory
import com.intellij.execution.configurations.LocatableConfigurationBase
import com.intellij.execution.configurations.RuntimeConfigurationError
import com.intellij.execution.runners.ExecutionEnvironment
import com.intellij.openapi.project.Project
import com.intellij.openapi.util.JDOMExternalizerUtil
import com.intellij.openapi.util.io.FileUtil
import org.jdom.Element
import java.io.File

/**
 * A single Run configuration. Two shapes:
 *   * **Project mode**  — [scriptPath] empty ⇒ runs `saule run` in [workingDirectory]
 *     (which must contain a `saule.config`).
 *   * **Single-file mode** — [scriptPath] set ⇒ runs `saule run <file> [args]`.
 */
class SauleRunConfiguration(
    project: Project,
    factory: ConfigurationFactory,
    name: String,
) : LocatableConfigurationBase<Any>(project, factory, name) {

    /** Path to a `.sau` file. Empty means "run the whole project" (project mode). */
    var scriptPath: String = ""

    /** Directory to launch from. Empty ⇒ auto (workspace root that owns the build output). */
    var workingDirectory: String = ""

    /** Extra args forwarded to the script via `Os.args()`. */
    var programArguments: String = ""

    /** Optional explicit path to the `saule` executable (else auto-discovered). */
    var exePath: String = ""

    override fun getConfigurationEditor() = SauleSettingsEditor(project)

    override fun getState(executor: Executor, environment: ExecutionEnvironment) =
        SauleCommandLineState(this, environment)

    override fun checkConfiguration() {
        if (scriptPath.isNotBlank() && !File(scriptPath).isFile) {
            throw RuntimeConfigurationError("Script file does not exist: $scriptPath")
        }
        if (workingDirectory.isNotBlank() && !File(workingDirectory).isDirectory) {
            throw RuntimeConfigurationError("Working directory does not exist: $workingDirectory")
        }
        if (exePath.isNotBlank() && !File(exePath).isFile) {
            throw RuntimeConfigurationError("Saule executable does not exist: $exePath")
        }
        if (scriptPath.isBlank()) {
            // Project mode needs a saule.config somewhere at/above the working dir.
            val wd = workingDirectory.ifBlank { project.basePath }
            if (wd != null && !hasConfigAtOrAbove(File(wd))) {
                throw RuntimeConfigurationError(
                    "Project mode needs a 'saule.config'. Pick a script file, or set a " +
                        "working directory inside a Saule project.",
                )
            }
        }
    }

    private fun hasConfigAtOrAbove(start: File): Boolean {
        var dir: File? = start
        var depth = 0
        while (dir != null && depth < 24) {
            if (File(dir, "saule.config").isFile) return true
            dir = dir.parentFile
            depth++
        }
        return false
    }

    override fun writeExternal(element: Element) {
        super.writeExternal(element)
        JDOMExternalizerUtil.writeField(element, SCRIPT, scriptPath)
        JDOMExternalizerUtil.writeField(element, WORKDIR, FileUtil.toSystemIndependentName(workingDirectory))
        JDOMExternalizerUtil.writeField(element, ARGS, programArguments)
        JDOMExternalizerUtil.writeField(element, EXE, FileUtil.toSystemIndependentName(exePath))
    }

    override fun readExternal(element: Element) {
        super.readExternal(element)
        scriptPath = JDOMExternalizerUtil.readField(element, SCRIPT, "")
        workingDirectory = JDOMExternalizerUtil.readField(element, WORKDIR, "")
        programArguments = JDOMExternalizerUtil.readField(element, ARGS, "")
        exePath = JDOMExternalizerUtil.readField(element, EXE, "")
    }

    private companion object {
        const val SCRIPT = "SCRIPT_PATH"
        const val WORKDIR = "WORKING_DIRECTORY"
        const val ARGS = "PROGRAM_ARGUMENTS"
        const val EXE = "EXE_PATH"
    }
}
