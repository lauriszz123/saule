package com.saule.lang

import com.intellij.notification.Notification
import com.intellij.notification.NotificationAction
import com.intellij.notification.NotificationGroupManager
import com.intellij.notification.NotificationType
import com.intellij.openapi.actionSystem.AnActionEvent
import com.intellij.openapi.options.ShowSettingsUtil
import com.intellij.openapi.project.Project

object SauleNotifications {

    private const val GROUP = "Saule"

    /** Warn that a toolchain binary couldn't be found, with a one-click jump to
     *  the settings page where the user can point at it. */
    fun warnMissingToolchain(project: Project?, binary: String) {
        val notification = NotificationGroupManager.getInstance()
            .getNotificationGroup(GROUP)
            .createNotification(
                "Saule toolchain not found",
                "Could not find <code>$binary</code>. Set the toolchain location to enable " +
                    "language features and running.",
                NotificationType.WARNING,
            )
        notification.addAction(object : NotificationAction("Configure…") {
            override fun actionPerformed(e: AnActionEvent, n: Notification) {
                ShowSettingsUtil.getInstance().showSettingsDialog(project, SauleConfigurable::class.java)
                n.expire()
            }
        })
        notification.notify(project)
    }
}
