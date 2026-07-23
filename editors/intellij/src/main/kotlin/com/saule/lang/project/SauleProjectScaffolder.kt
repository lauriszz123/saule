package com.saule.lang.project

import java.io.File

/**
 * Writes the initial files of a Saule project into [root].
 *
 * Mirrors `saule init` (`crates/saule-cli/src/init.rs`) so a project created in
 * the IDE is byte-for-byte what the CLI would produce: `saule.config`,
 * `src/main.sau`, `.gitignore`, and a `README.md`.
 */
object SauleProjectScaffolder {

    /** Toolchain version written into `min_saule_version`. Kept in sync with the
     *  workspace `Cargo.toml` (`workspace.package.version`). */
    const val SAULE_VERSION = "2026.1.0"

    fun scaffold(root: File, projectName: String) {
        File(root, "src").mkdirs()

        write(root, "saule.config", config(projectName))
        write(root, "src/main.sau", MAIN_SAU)
        write(root, ".gitignore", GITIGNORE)
        write(root, "README.md", readme(projectName))
    }

    private fun write(root: File, relPath: String, contents: String) {
        val target = File(root, relPath)
        target.parentFile?.mkdirs()
        // Don't clobber files a user template may already have placed here.
        if (!target.exists()) target.writeText(contents)
    }

    private fun config(name: String): String = buildString {
        appendLine("name: \"$name\"")
        appendLine("version: \"0.1.0\"")
        appendLine("entry: \"src/main.sau\"")
        appendLine("src_dirs: [\"src\"]")
        appendLine("min_saule_version: \"$SAULE_VERSION\"")
    }

    private fun readme(name: String): String =
        "# $name\n\nA Saule project. Run with:\n\n```sh\nsaule run\n```\n"

    private const val GITIGNORE = "*.log\n"

    private val MAIN_SAU = """
        --[[
        Entry point.

        The `Main` class with a `static fn main()` is the default entry point for a Saule.
        ]]

        class Greeter
            local who: string

            fn init(who: string)
                self.who = who
            end

            fn greet()
                println("Hello, " .. self.who)
            end
        end

        class Main
            static fn main()
                local g: Greeter = Greeter("world")
                g.greet()
            end
        end
    """.trimIndent() + "\n"
}
