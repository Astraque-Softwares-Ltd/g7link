// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.toggles

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

/**
 * A PC with no internet asked for this phone's hotspot and the phone can't turn
 * it on itself (Elevated isn't running): one notification asks the user, and a
 * tap opens the hotspot settings. The PC sees the hotspot come on from the
 * phone's toggles.
 */
object HotspotAsk {
    private const val CHANNEL = "hotspot_requests"
    private const val TAG = "hotspot_ask"
    private const val ID = 1
    private const val ASK_FOR_MS = 2 * 60 * 1000L

    /** The screen where the user turns the hotspot on. */
    fun settingsIntent(): Intent =
        Intent("android.settings.TETHER_SETTINGS").addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)

    /** Shows the request; false if notifications are off. */
    fun show(context: Context): Boolean {
        if (ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) !=
            PackageManager.PERMISSION_GRANTED
        ) {
            return false
        }
        context.getSystemService(NotificationManager::class.java)?.createNotificationChannel(
            NotificationChannel(
                CHANNEL,
                context.getString(R.string.channel_hotspot_requests),
                NotificationManager.IMPORTANCE_HIGH,
            ),
        )
        val open = PendingIntent.getActivity(
            context,
            ID,
            settingsIntent(),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val notification = NotificationCompat.Builder(context, CHANNEL)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(context.getString(R.string.hotspot_ask_title))
            .setContentText(context.getString(R.string.hotspot_ask_text))
            .setContentIntent(open)
            .addAction(0, context.getString(R.string.hotspot_ask_open), open)
            .setAutoCancel(true)
            .setTimeoutAfter(ASK_FOR_MS)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .build()
        return runCatching { NotificationManagerCompat.from(context).notify(TAG, ID, notification) }.isSuccess
    }

    fun dismiss(context: Context) {
        runCatching { NotificationManagerCompat.from(context).cancel(TAG, ID) }
    }
}
