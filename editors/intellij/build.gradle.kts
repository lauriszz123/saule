import org.jetbrains.intellij.platform.gradle.IntelliJPlatformType
import org.jetbrains.intellij.platform.gradle.TestFrameworkType

plugins {
    id("java")
    id("org.jetbrains.kotlin.jvm") version "2.0.21"
    // IntelliJ Platform Gradle Plugin 2.x
    id("org.jetbrains.intellij.platform") version "2.1.0"
}

group = providers.gradleProperty("pluginGroup").get()
version = providers.gradleProperty("pluginVersion").get()

repositories {
    mavenCentral()
    intellijPlatform {
        defaultRepositories()
    }
}

dependencies {
    intellijPlatform {
        create(
            providers.gradleProperty("platformType").get(),
            providers.gradleProperty("platformVersion").get(),
        )

        // LSP4IJ — the LSP client framework we build on.
        plugin("com.redhat.devtools.lsp4ij:${providers.gradleProperty("lsp4ijVersion").get()}")

        // Bundled TextMate support is used as a fallback highlighter host if
        // you prefer the TextMate grammar over the native lexer (optional).
        bundledPlugin("org.jetbrains.plugins.textmate")

        instrumentationTools()
        pluginVerifier()

        // Puts the platform (and JUnit 4) on the test classpath. The indent
        // model tests need nothing more than that — no IDE fixture.
        testFramework(TestFrameworkType.Platform)
    }

    testImplementation("junit:junit:4.13.2")
}

tasks.test {
    useJUnit()
}

intellijPlatform {
    pluginConfiguration {
        version = providers.gradleProperty("pluginVersion")
        ideaVersion {
            sinceBuild = providers.gradleProperty("pluginSinceBuild")
            // Leave the upper bound open so the plugin keeps loading on newer IDEs.
            untilBuild = provider { null }
        }
    }

    // `verifyPlugin` runs the JetBrains Plugin Verifier. Point it at the same
    // platform we compile against so it reuses the already-downloaded IDE
    // instead of fetching the whole `recommended()` matrix.
    pluginVerification {
        ides {
            ide(
                providers.gradleProperty("platformType"),
                providers.gradleProperty("platformVersion"),
            )
        }
    }
}

kotlin {
    jvmToolchain(providers.gradleProperty("javaVersion").get().toInt())
}

java {
    val v = providers.gradleProperty("javaVersion").get().toInt()
    sourceCompatibility = JavaVersion.toVersion(v)
    targetCompatibility = JavaVersion.toVersion(v)
}
