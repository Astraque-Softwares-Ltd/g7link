// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.links

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import androidx.core.net.toUri
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import app.g7link.android.R
import app.g7link.core.ClipKind
import app.g7link.core.ClipSuggestion
import app.g7link.core.mapsWebUrl
import app.g7link.core.trackingSearchUrl

/**
 * Links and smart clipboard actions from a PC: Android doesn't let an app in
 * the background open screens, so links and background clip suggestions wait
 * in a notification on the same channel.
 */
object LinkNotifications {
    /** Also Play on phone's offers ([app.g7link.android.media.WatchOffers]). */
    internal const val CHANNEL = "links"
    private const val TAG = "link"
    private const val CLIP_TAG = "clip_suggestion"
    private const val CLIP_NOTIFICATION_ID = 0x434C4950 // "CLIP"
    private val WHITESPACE = Regex("\\s+")

    fun createChannel(context: Context) {
        context.getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(CHANNEL, context.getString(R.string.channel_links), NotificationManager.IMPORTANCE_HIGH),
        )
    }

    /** Shows a link, video with timestamp, or map location from a PC; false if notifications are off. */
    fun show(context: Context, pcName: String, url: String): Boolean {
        if (ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) !=
            PackageManager.PERMISSION_GRANTED
        ) {
            return false
        }
        val handoff = app.g7link.core.extractHandoffLink(url)
        val viewIntent = if (url.startsWith("geo:", ignoreCase = true)) {
            val geoIntent = Intent(Intent.ACTION_VIEW, url.toUri()).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            if (geoIntent.resolveActivity(context.packageManager) != null) {
                geoIntent
            } else {
                val webFallback = app.g7link.core.geoToMapsHttps(url) ?: url
                Intent(Intent.ACTION_VIEW, webFallback.toUri()).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            }
        } else {
            Intent(Intent.ACTION_VIEW, url.toUri()).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        }
        // When G7 Link is in the foreground, Android allows opening the target right away.
        runCatching { context.startActivity(viewIntent) }

        val open = PendingIntent.getActivity(
            context,
            url.hashCode(),
            viewIntent,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val from = pcName.ifEmpty { context.getString(R.string.your_pc) }
        val title = when (handoff?.kind) {
            app.g7link.core.HandoffKind.MAP_LOCATION -> context.getString(R.string.handoff_map_from, from)
            app.g7link.core.HandoffKind.VIDEO_LINK -> context.getString(R.string.handoff_video_from, from)
            else -> context.getString(R.string.link_from, from)
        }
        val shown = handoff?.label ?: withoutScheme(url)
        val actionLabel = when (handoff?.kind) {
            app.g7link.core.HandoffKind.MAP_LOCATION -> context.getString(R.string.clip_action_maps)
            app.g7link.core.HandoffKind.VIDEO_LINK -> context.getString(R.string.handoff_action_continue_video)
            else -> context.getString(R.string.clip_action_open)
        }
        val notification = NotificationCompat.Builder(context, CHANNEL)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(title)
            .setContentText(shown)
            .setStyle(NotificationCompat.BigTextStyle().bigText(shown))
            .setContentIntent(open)
            .addAction(0, actionLabel, open)
            .setAutoCancel(true)
            .setCategory(NotificationCompat.CATEGORY_RECOMMENDATION)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .build()
        return runCatching { NotificationManagerCompat.from(context).notify(TAG, url.hashCode(), notification) }.isSuccess
    }

    /** A link as the app shows it: no scheme, no trailing slash. */
    internal fun withoutScheme(url: String): String = url.substringAfter("://").removeSuffix("/")

    /** "<What> copied from %1$s" for a [ClipKind]. */
    internal fun clipTitleRes(kind: ClipKind): Int = when (kind) {
        ClipKind.WEB_LINK -> R.string.clip_copied_link_from
        ClipKind.STREET_ADDRESS -> R.string.clip_copied_address_from
        ClipKind.PHONE_NUMBER -> R.string.clip_copied_phone_from
        ClipKind.TRACKING_NUMBER -> R.string.clip_copied_tracking_from
        ClipKind.EMAIL -> R.string.clip_copied_email_from
    }

    /** What the button does for a [ClipKind], matching [intentForSuggestion]. */
    internal fun clipActionRes(kind: ClipKind): Int = when (kind) {
        ClipKind.WEB_LINK -> R.string.clip_action_open_link
        ClipKind.STREET_ADDRESS -> R.string.clip_action_maps
        ClipKind.PHONE_NUMBER -> R.string.clip_action_call
        ClipKind.TRACKING_NUMBER -> R.string.clip_action_track
        ClipKind.EMAIL -> R.string.clip_action_write_email
    }

    /** The copied target on one line; a link without its scheme. */
    internal fun clipShownTarget(kind: ClipKind, target: String): String {
        val line = target.replace(WHITESPACE, " ").trim()
        return if (kind == ClipKind.WEB_LINK) withoutScheme(line) else line
    }

    fun clipTitle(context: Context, kind: ClipKind, from: String): String =
        context.getString(clipTitleRes(kind), from)

    fun actionLabel(context: Context, kind: ClipKind): String = context.getString(clipActionRes(kind))

    /** Builds the Android [Intent] for a [ClipSuggestion]. */
    fun intentForSuggestion(context: Context, suggestion: ClipSuggestion): Intent = when (suggestion.kind) {
        ClipKind.WEB_LINK -> Intent(Intent.ACTION_VIEW, suggestion.target.toUri())
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        ClipKind.STREET_ADDRESS -> {
            val geoIntent = Intent(
                Intent.ACTION_VIEW,
                Uri.parse("geo:0,0?q=${Uri.encode(suggestion.target)}"),
            ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            if (geoIntent.resolveActivity(context.packageManager) != null) {
                geoIntent
            } else {
                Intent(Intent.ACTION_VIEW, mapsWebUrl(suggestion.target).toUri())
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            }
        }
        ClipKind.PHONE_NUMBER -> Intent(
            Intent.ACTION_DIAL,
            Uri.fromParts("tel", suggestion.target, null),
        ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        ClipKind.TRACKING_NUMBER -> Intent(
            Intent.ACTION_VIEW,
            trackingSearchUrl(suggestion.target).toUri(),
        ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        ClipKind.EMAIL -> Intent(
            Intent.ACTION_SENDTO,
            Uri.fromParts("mailto", suggestion.target, null),
        ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    }

    /** Runs a [ClipSuggestion] action on this phone, with web/view fallback if needed. */
    fun runSuggestion(context: Context, suggestion: ClipSuggestion) {
        dismissClipSuggestion(context)
        val primary = intentForSuggestion(context, suggestion)
        if (runCatching { context.startActivity(primary) }.isSuccess) return
        val fallback = when (suggestion.kind) {
            ClipKind.STREET_ADDRESS -> Intent(Intent.ACTION_VIEW, mapsWebUrl(suggestion.target).toUri())
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            ClipKind.EMAIL -> Intent(Intent.ACTION_VIEW, Uri.fromParts("mailto", suggestion.target, null))
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            else -> null
        }
        if (fallback != null) {
            runCatching { context.startActivity(fallback) }
        }
    }

    /** Shows a silent notification with the suggested action chip for a received clip. */
    fun showClipSuggestion(context: Context, pcName: String, suggestion: ClipSuggestion): Boolean {
        if (ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) !=
            PackageManager.PERMISSION_GRANTED
        ) {
            return false
        }
        val intent = intentForSuggestion(context, suggestion)
        val pending = PendingIntent.getActivity(
            context,
            CLIP_NOTIFICATION_ID,
            intent,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val label = actionLabel(context, suggestion.kind)
        val from = pcName.ifEmpty { context.getString(R.string.your_pc) }
        val lockScreen = NotificationCompat.Builder(context, CHANNEL)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(context.getString(R.string.clip_received_from, from))
            .setCategory(NotificationCompat.CATEGORY_RECOMMENDATION)
            .build()
        val notification = NotificationCompat.Builder(context, CHANNEL)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(clipTitle(context, suggestion.kind, from))
            .setContentText(clipShownTarget(suggestion.kind, suggestion.target))
            .setContentIntent(pending)
            .addAction(0, label, pending)
            .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
            .setPublicVersion(lockScreen)
            .setSilent(true)
            .setAutoCancel(true)
            .setCategory(NotificationCompat.CATEGORY_RECOMMENDATION)
            .setPriority(NotificationCompat.PRIORITY_DEFAULT)
            .build()
        return runCatching {
            NotificationManagerCompat.from(context).notify(CLIP_TAG, CLIP_NOTIFICATION_ID, notification)
        }.isSuccess
    }

    fun dismissClipSuggestion(context: Context) {
        runCatching { NotificationManagerCompat.from(context).cancel(CLIP_TAG, CLIP_NOTIFICATION_ID) }
    }
}

