// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.elevated

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import app.g7link.android.R
import app.g7link.android.ui.MainActivity

/**
 * Says, once, that Elevated is paused on a Wi-Fi network the user hasn't
 * trusted; tapping it opens Elevated's settings, where Trust this network is.
 */
object NetworkPausedNotification {
    private const val CHANNEL = "elevated"
    private const val ID = 47

    /** False if notifications are off for the app. */
    fun show(context: Context, network: WifiNetwork): Boolean {
        if (ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
            return false
        }
        context.getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(CHANNEL, context.getString(R.string.channel_elevated), NotificationManager.IMPORTANCE_HIGH),
        )
        val open = PendingIntent.getActivity(
            context,
            ID,
            Intent(context, MainActivity::class.java)
                .putExtra(MainActivity.EXTRA_OPEN_ELEVATED, true)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val text = context.getString(R.string.elevated_paused_notice, network.address)
        val notification = NotificationCompat.Builder(context, CHANNEL)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(context.getString(R.string.elevated_paused_title))
            .setContentText(text)
            .setStyle(NotificationCompat.BigTextStyle().bigText(text))
            .setContentIntent(open)
            .setAutoCancel(true)
            .setOnlyAlertOnce(true)
            .build()
        return runCatching { NotificationManagerCompat.from(context).notify(ID, notification) }.isSuccess
    }

    fun dismiss(context: Context) = NotificationManagerCompat.from(context).cancel(ID)
}
