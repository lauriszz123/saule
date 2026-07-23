package com.saule.lang

import com.intellij.openapi.diagnostic.logger
import com.intellij.openapi.project.Project
import com.intellij.openapi.roots.ProjectRootManager
import com.intellij.openapi.util.SystemInfo
import java.io.File
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.Paths

/**
 * Locates the Saule toolchain binaries (`saule`, `saule-lsp`) and the workspace
 * root to run them from.
 *
 * The binaries are the Cargo build output of the Saule repo, so resolution
 * walks UP from the project base path and every content root looking for
 * `target/{release,debug}/<name>[.exe]`. That way opening a sub-folder (e.g.
 * `examples/json_usage`) still finds the workspace-root build output, and the
 * server / CLI is launched with its working directory set to that root.
 */
object SauleToolchain {
    private val LOG = logger<SauleToolchain>()

    /**
     * @property command    the executable path (or bare name if nothing was found).
     * @property workingDir where to run it (may be null).
     * @property found      true if [command] resolved to a real file / PATH entry;
     *                      false means we fell back to the bare name.
     */
    data class Located(val command: String, val workingDir: String?, val found: Boolean)

    /** Windows needs the `.exe` suffix; Unix does not. */
    fun exeName(base: String): String = if (SystemInfo.isWindows) "$base.exe" else base

    /**
     * @param base           bare binary name, e.g. `"saule"` or `"saule-lsp"`.
     * @param overrideEnv    env var that, if set, pins the executable path.
     * @param overrideProp   system property that, if set, pins the executable path.
     */
    fun locate(
        project: Project,
        base: String,
        overrideEnv: String? = null,
        overrideProp: String? = null,
    ): Located {
        val exe = exeName(base)

        // 1. Environment / system-property override.
        explicitOverride(overrideEnv, overrideProp)?.let {
            return Located(it, project.basePath, found = true)
        }

        // 2. Configured toolchain in Settings ▸ Languages & Frameworks ▸ Saule.
        val settings = SauleSettings.getInstance()
        settings.explicitPathFor(base).trim().ifEmpty { null }?.let {
            return Located(it, project.basePath, found = File(it).isFile)
        }
        settings.toolchainDir.trim().ifEmpty { null }?.let { dir ->
            val candidate = Paths.get(dir, exe)
            if (Files.isRegularFile(candidate)) {
                return Located(candidate.toString(), project.basePath, found = true)
            }
        }

        // 3. Cargo build output, walking up from the project / content roots.
        for (root in candidateRoots(project)) {
            findUpward(root, exe)?.let { (path, workspace) ->
                LOG.info("Using $base from build output: $path (cwd=$workspace)")
                return Located(path.toString(), workspace.toString(), found = true)
            }
        }

        // 4. PATH.
        findOnPath(exe)?.let {
            LOG.info("Using $base from PATH: $it")
            return Located(it.toString(), project.basePath, found = true)
        }

        // 5. Nothing found — bare name, so callers can surface a helpful error.
        LOG.warn("$base not found via settings, target/ dirs, or PATH")
        return Located(exe, project.basePath, found = false)
    }

    /** Walk up from [start], returning the `target/{release,debug}/exe` and the
     *  directory that contains that `target/` folder (the workspace root). */
    private fun findUpward(start: Path, exe: String): Pair<Path, Path>? {
        var dir: Path? = start
        var depth = 0
        while (dir != null && depth < 24) {
            for (profile in listOf("release", "debug")) {
                val candidate = dir.resolve("target").resolve(profile).resolve(exe)
                if (Files.isRegularFile(candidate)) return candidate to dir
            }
            dir = dir.parent
            depth++
        }
        return null
    }

    private fun candidateRoots(project: Project): List<Path> {
        val roots = LinkedHashSet<Path>()
        project.basePath?.let { runCatching { roots.add(Paths.get(it)) } }
        runCatching {
            ProjectRootManager.getInstance(project).contentRoots.forEach { vf ->
                runCatching { roots.add(Paths.get(vf.path)) }
            }
        }
        return roots.toList()
    }

    private fun explicitOverride(env: String?, prop: String?): String? {
        val raw = prop?.let { System.getProperty(it) }
            ?: env?.let { System.getenv(it) }
            ?: return null
        return raw.trim().ifEmpty { null }
    }

    private fun findOnPath(exe: String): Path? {
        val pathEnv = System.getenv("PATH") ?: return null
        return pathEnv.split(File.pathSeparatorChar)
            .asSequence()
            .filter { it.isNotBlank() }
            .map { Paths.get(it, exe) }
            .firstOrNull { Files.isRegularFile(it) }
    }
}
