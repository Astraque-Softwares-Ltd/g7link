// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.notifications

import android.Manifest
import android.annotation.SuppressLint
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.RemoteInput
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.drawable.Icon
import android.net.Uri
import android.os.Build
import androidx.annotation.StringRes
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import app.g7link.android.R
import app.g7link.android.ui.MainActivity
import app.g7link.android.ui.agents.AgentsModel
import app.g7link.android.ui.agents.AgentsState
import app.g7link.android.ui.agents.ThreadStatus
import app.g7link.core.AgentAsk
import app.g7link.core.AgentOutcome
import app.g7link.core.Event
import java.util.Date
import java.util.concurrent.ConcurrentHashMap

/** What a thread's one notification should become. */
sealed interface AgentNotice {
    /** A turn running in a thread this phone started or opened. */
    data class Working(val pcId: String, val threadId: String, val agent: String, val folder: String, val title: String) : AgentNotice

    /** A request waiting for an answer; [allowable] when it can be allowed without opening the app. */
    data class Waiting(
        val pcId: String,
        val threadId: String,
        val ask: AgentAsk,
        val agent: String,
        /** The folder's label, when the thread is known. */
        val folder: String?,
        val allowable: Boolean,
    ) : AgentNotice

    /** A turn that ended done, failed or paused at a usage limit. */
    data class Finished(
        val pcId: String,
        val threadId: String,
        val agent: String,
        val title: String,
        val kind: FinishKind,
        /** Done: the summary. Failed: the CLI's message. */
        val text: String?,
        val resetsMs: Long? = null,
    ) : AgentNotice

    data class Cancel(val pcId: String, val threadId: String) : AgentNotice

    /** Everything of a PC's: its session ended or nothing can run there now. */
    data class CancelPc(val pcId: String) : AgentNotice
}

enum class FinishKind { DONE, FAILED, PAUSED }

/** A notification's button, and whether Android must have the phone unlocked to send it. */
data class NoticeAction(val kind: Kind, val authenticationRequired: Boolean) {
    enum class Kind { DENY, ALLOW, REVIEW, STOP, REPLY }
}

/**
 * A PC's agent threads on this phone's notifications (docs/protocol/agents.md):
 * one per thread, replaced in place, by tag `agent:<pc>:<thread>`. A request
 * waiting for an answer hides its command on the lock screen; Allow and Reply
 * need the phone unlocked, Deny and Stop don't.
 */
object AgentNotifications {
    private const val CHANNEL_WORKING = "agents_working"
    private const val CHANNEL_WAITING = "agents_waiting"
    private const val CHANNEL_DONE = "agents_done"
    private const val NOTIFICATION_ID = 0x4147_4E54 // "AGNT"
    private const val TAG_PREFIX = "agent:"

    /** The reply typed under a finished thread's notification. */
    const val REPLY_KEY = "agent_reply"

    /** Android lets an action require an unlocked phone from 12 (S) on; earlier, Allow and Reply aren't offered. */
    private const val AUTH_SDK = 31

    /** How much of a command or change shows in the expanded notification. */
    private const val MAX_BODY_CHARS = 600

    /** When each thread was first seen working, for its "working for" timer. */
    private val workingSince = ConcurrentHashMap<String, Long>()

    /** The request each notification last showed, so a repost of the same one doesn't sound again. */
    private val shownAsk = ConcurrentHashMap<String, String>()

    fun tag(pcId: String, threadId: String): String = "$TAG_PREFIX$pcId:$threadId"

    fun createChannels(context: Context) {
        val manager = context.getSystemService(NotificationManager::class.java) ?: return
        manager.createNotificationChannel(
            NotificationChannel(
                CHANNEL_WORKING,
                context.getString(R.string.channel_agents_working),
                NotificationManager.IMPORTANCE_LOW,
            ).apply { setShowBadge(false) },
        )
        manager.createNotificationChannel(
            NotificationChannel(
                CHANNEL_WAITING,
                context.getString(R.string.channel_agents_waiting),
                NotificationManager.IMPORTANCE_HIGH,
            ).apply { lockscreenVisibility = Notification.VISIBILITY_PRIVATE },
        )
        manager.createNotificationChannel(
            NotificationChannel(
                CHANNEL_DONE,
                context.getString(R.string.channel_agents_done),
                NotificationManager.IMPORTANCE_DEFAULT,
            ),
        )
    }

    /**
     * What one of a PC's agent events does to the notifications. [state] is the
     * PC's state after the event; [shown] the thread whose screen is in front
     * (nothing posts for it); [followed] whether this phone started or opened a
     * thread (only those show while they work).
     */
    fun plan(
        pcId: String,
        event: Event,
        state: AgentsState,
        shown: String?,
        followed: (String) -> Boolean,
    ): List<AgentNotice> = when (event) {
        is Event.AgentCatalog -> if (event.catalog == null) listOf(AgentNotice.CancelPc(pcId)) else emptyList()
        is Event.AgentThreadRemoved -> listOf(AgentNotice.Cancel(pcId, event.thread))
        is Event.AgentAsk -> listOfNotNull(threadNotice(pcId, state, event.ask.thread, shown, followed))
        is Event.AgentAsked -> listOfNotNull(threadNotice(pcId, state, event.thread, shown, followed))
        // A turn's end is AgentDone's; a change while it runs can bring the working one back.
        is Event.AgentThreadChanged ->
            if (AgentsModel.running(event.thread)) listOfNotNull(threadNotice(pcId, state, event.thread.id, shown, followed)) else emptyList()
        is Event.AgentDone -> listOf(finished(pcId, state, event, shown))
        else -> emptyList()
    }

    /**
     * What a thread's notification should show now: its oldest waiting request,
     * else that it's working (when followed), else nothing. Null leaves it as it
     * is (a finished turn's notification stays until it's dismissed).
     */
    fun threadNotice(
        pcId: String,
        state: AgentsState,
        threadId: String,
        shown: String?,
        followed: (String) -> Boolean,
    ): AgentNotice? {
        if (threadId == shown) return null
        val thread = state.thread(threadId)
        val ask = state.asksFor(threadId).firstOrNull()
        if (ask != null) {
            return AgentNotice.Waiting(
                pcId = pcId,
                threadId = threadId,
                ask = ask,
                agent = state.agentName(thread?.agent ?: ask.agent),
                folder = thread?.folder?.takeIf { it.isNotBlank() },
                allowable = AgentsModel.allowableInNotification(ask),
            )
        }
        if (thread == null) return AgentNotice.Cancel(pcId, threadId)
        if (!AgentsModel.running(thread)) return null
        return if (followed(threadId)) {
            AgentNotice.Working(pcId, threadId, state.agentName(thread.agent), thread.folder, thread.title)
        } else {
            AgentNotice.Cancel(pcId, threadId)
        }
    }

    private fun finished(pcId: String, state: AgentsState, event: Event.AgentDone, shown: String?): AgentNotice {
        val thread = event.thread
        if (thread.id == shown) return AgentNotice.Cancel(pcId, thread.id)
        val agent = state.agentName(thread.agent)
        return when (event.outcome) {
            AgentOutcome.INTERRUPTED -> AgentNotice.Cancel(pcId, thread.id)
            AgentOutcome.DONE -> AgentNotice.Finished(
                pcId,
                thread.id,
                agent,
                thread.title,
                FinishKind.DONE,
                event.summary.ifBlank { thread.summary.orEmpty() }.ifBlank { null },
            )
            AgentOutcome.FAILED -> when (val status = AgentsModel.failure(thread.error)) {
                is ThreadStatus.Paused -> AgentNotice.Finished(pcId, thread.id, agent, thread.title, FinishKind.PAUSED, null, status.resetsMs)
                else -> AgentNotice.Finished(pcId, thread.id, agent, thread.title, FinishKind.FAILED, AgentsModel.shownError(thread.error))
            }
        }
    }

    /** A waiting request's buttons: Deny first, then Allow (unlocked phones only) or Review. */
    fun waitingActions(allowable: Boolean, sdkInt: Int): List<NoticeAction> = listOf(
        NoticeAction(NoticeAction.Kind.DENY, authenticationRequired = false),
        if (allowable && sdkInt >= AUTH_SDK) {
            NoticeAction(NoticeAction.Kind.ALLOW, authenticationRequired = true)
        } else {
            NoticeAction(NoticeAction.Kind.REVIEW, authenticationRequired = false)
        },
    )

    /** On the lock screen: nothing that shows the command, and nothing that needs it unlocked. */
    fun publicWaitingActions(): List<NoticeAction> = listOf(
        NoticeAction(NoticeAction.Kind.DENY, authenticationRequired = false),
        NoticeAction(NoticeAction.Kind.REVIEW, authenticationRequired = false),
    )

    fun workingActions(): List<NoticeAction> = listOf(NoticeAction(NoticeAction.Kind.STOP, authenticationRequired = false))

    /** A finished turn can be replied to from the notification, unlocked; a paused one can't yet. */
    fun finishedActions(kind: FinishKind, sdkInt: Int): List<NoticeAction> =
        if (kind != FinishKind.PAUSED && sdkInt >= AUTH_SDK) {
            listOf(NoticeAction(NoticeAction.Kind.REPLY, authenticationRequired = true))
        } else {
            emptyList()
        }

    /** Posts and cancels what [plan] or [threadNotice] decided. */
    fun apply(context: Context, pcName: String, notices: List<AgentNotice>) {
        for (notice in notices) {
            when (notice) {
                is AgentNotice.Cancel -> cancel(context, notice.pcId, notice.threadId)
                is AgentNotice.CancelPc -> cancelPc(context, notice.pcId)
                is AgentNotice.Working -> post(context, tag(notice.pcId, notice.threadId), working(context, pcName, notice))
                is AgentNotice.Waiting -> {
                    val tag = tag(notice.pcId, notice.threadId)
                    val again = shownAsk[tag] == notice.ask.token
                    shownAsk[tag] = notice.ask.token
                    post(context, tag, waiting(context, pcName, notice, again))
                }
                is AgentNotice.Finished -> {
                    val tag = tag(notice.pcId, notice.threadId)
                    workingSince.remove(tag)
                    shownAsk.remove(tag)
                    post(context, tag, finished(context, pcName, notice))
                }
            }
        }
    }

    fun cancel(context: Context, pcId: String, threadId: String) {
        val tag = tag(pcId, threadId)
        workingSince.remove(tag)
        shownAsk.remove(tag)
        runCatching { NotificationManagerCompat.from(context).cancel(tag, NOTIFICATION_ID) }
    }

    fun cancelPc(context: Context, pcId: String) {
        val prefix = "$TAG_PREFIX$pcId:"
        workingSince.keys.removeAll { it.startsWith(prefix) }
        shownAsk.keys.removeAll { it.startsWith(prefix) }
        val manager = context.getSystemService(NotificationManager::class.java) ?: return
        runCatching {
            manager.activeNotifications
                .filter { it.id == NOTIFICATION_ID && it.tag?.startsWith(prefix) == true }
                .forEach { manager.cancel(it.tag, NOTIFICATION_ID) }
        }
    }

    /** An answer, Stop or reply from a notification didn't go through: say so in its place. */
    fun failed(context: Context, pcId: String, threadId: String, @StringRes message: Int) {
        val tag = tag(pcId, threadId)
        workingSince.remove(tag)
        shownAsk.remove(tag)
        val notification = Notification.Builder(context, CHANNEL_DONE)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(context.getString(message))
            .setContentIntent(openIntent(context, pcId, threadId))
            .setAutoCancel(true)
            .setShowWhen(true)
            .setWhen(System.currentTimeMillis())
            .setCategory(Notification.CATEGORY_ERROR)
            .build()
        post(context, tag, notification)
    }

    @SuppressLint("NewApi")
    private fun working(context: Context, pcName: String, notice: AgentNotice.Working): Notification {
        val tag = tag(notice.pcId, notice.threadId)
        val since = workingSince.getOrPut(tag) { System.currentTimeMillis() }
        val builder = Notification.Builder(context, CHANNEL_WORKING)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(context.getString(R.string.agents_notify_working, notice.agent, notice.folder))
            .setContentText(notice.title)
            .setSubText(pcName)
            .setContentIntent(openIntent(context, notice.pcId, notice.threadId))
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setShowWhen(true)
            .setWhen(since)
            .setUsesChronometer(true)
            .setCategory(Notification.CATEGORY_PROGRESS)
        for (action in workingActions()) builder.addAction(action(context, action, notice.pcId, notice.threadId, null))
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.BAKLAVA) {
            runCatching { builder.setRequestPromotedOngoing(true) }
            builder.setShortCriticalText(notice.agent)
        }
        return builder.build()
    }

    private fun waiting(context: Context, pcName: String, notice: AgentNotice.Waiting, again: Boolean): Notification {
        val ask = notice.ask
        val open = openIntent(context, notice.pcId, notice.threadId)
        val publicTitle = context.getString(R.string.agents_notify_waiting_public, notice.agent, pcName)
        val public = Notification.Builder(context, CHANNEL_WAITING)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(publicTitle)
            .setContentIntent(open)
            .setCategory(Notification.CATEGORY_MESSAGE)
        for (action in publicWaitingActions()) public.addAction(action(context, action, notice.pcId, notice.threadId, ask.token))
        val title = if (notice.folder != null) {
            context.getString(R.string.agents_notify_waiting_title, notice.agent, notice.folder)
        } else {
            publicTitle
        }
        val body = AgentsModel.askBody(ask) ?: ask.path
        val expanded = listOfNotNull(ask.title.ifBlank { null }, body?.take(MAX_BODY_CHARS)).joinToString("\n")
        val now = System.currentTimeMillis()
        val builder = Notification.Builder(context, CHANNEL_WAITING)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(title)
            .setContentText(ask.title)
            .setStyle(Notification.BigTextStyle().bigText(expanded))
            .setSubText(pcName)
            .setContentIntent(open)
            .setCategory(Notification.CATEGORY_MESSAGE)
            .setVisibility(Notification.VISIBILITY_PRIVATE)
            .setPublicVersion(public.build())
            .setOnlyAlertOnce(again)
            .setShowWhen(true)
            .setWhen(now)
        // The request lapses on the PC after its time; so does its notification.
        if (ask.expiresMs > now) builder.setTimeoutAfter(ask.expiresMs - now)
        for (action in waitingActions(notice.allowable, Build.VERSION.SDK_INT)) {
            builder.addAction(action(context, action, notice.pcId, notice.threadId, ask.token))
        }
        return builder.build()
    }

    private fun finished(context: Context, pcName: String, notice: AgentNotice.Finished): Notification {
        val title = when (notice.kind) {
            FinishKind.DONE -> context.getString(R.string.agents_notify_done_title, notice.agent)
            FinishKind.FAILED -> context.getString(R.string.agents_notify_failed_title, notice.agent)
            FinishKind.PAUSED -> context.getString(R.string.agents_notify_paused_title, notice.agent)
        }
        val detail = when (notice.kind) {
            FinishKind.PAUSED -> notice.resetsMs?.let {
                context.getString(R.string.agents_usage_resets, android.text.format.DateFormat.getTimeFormat(context).format(Date(it)))
            } ?: context.getString(R.string.agents_usage_limit)
            else -> notice.text
        }
        val expanded = listOfNotNull(notice.title.ifBlank { null }, detail?.ifBlank { null }).joinToString("\n")
        val builder = Notification.Builder(context, CHANNEL_DONE)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(title)
            .setContentText(notice.title)
            .setStyle(Notification.BigTextStyle().bigText(expanded))
            .setSubText(pcName)
            .setContentIntent(openIntent(context, notice.pcId, notice.threadId))
            .setAutoCancel(true)
            .setShowWhen(true)
            .setWhen(System.currentTimeMillis())
            .setCategory(if (notice.kind == FinishKind.DONE) Notification.CATEGORY_STATUS else Notification.CATEGORY_ERROR)
        for (action in finishedActions(notice.kind, Build.VERSION.SDK_INT)) {
            builder.addAction(action(context, action, notice.pcId, notice.threadId, null))
        }
        return builder.build()
    }

    @SuppressLint("NewApi")
    private fun action(context: Context, spec: NoticeAction, pcId: String, threadId: String, token: String?): Notification.Action {
        val icon = Icon.createWithResource(context, R.drawable.ic_notification)
        val (label, intent) = when (spec.kind) {
            NoticeAction.Kind.REVIEW -> context.getString(R.string.agents_review) to openIntent(context, pcId, threadId)
            NoticeAction.Kind.DENY -> context.getString(R.string.agents_deny) to
                broadcast(context, AgentActionReceiver.ACTION_DENY, pcId, threadId, token, mutable = false)
            NoticeAction.Kind.ALLOW -> context.getString(R.string.agents_allow) to
                broadcast(context, AgentActionReceiver.ACTION_ALLOW, pcId, threadId, token, mutable = false)
            NoticeAction.Kind.STOP -> context.getString(R.string.agents_stop) to
                broadcast(context, AgentActionReceiver.ACTION_STOP, pcId, threadId, null, mutable = false)
            // RemoteInput fills in the typed text, so this one intent has to be mutable (it's explicit).
            NoticeAction.Kind.REPLY -> context.getString(R.string.agents_notify_reply) to
                broadcast(context, AgentActionReceiver.ACTION_REPLY, pcId, threadId, null, mutable = true)
        }
        val builder = Notification.Action.Builder(icon, label, intent)
        if (spec.kind == NoticeAction.Kind.REPLY) {
            builder.addRemoteInput(RemoteInput.Builder(REPLY_KEY).setLabel(label).build())
            builder.setAllowGeneratedReplies(false)
        }
        if (spec.authenticationRequired && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            builder.setAuthenticationRequired(true)
        }
        return builder.build()
    }

    /** Opens the thread in the app. Each thread has its own (the data URI), so one never takes another's extras. */
    private fun openIntent(context: Context, pcId: String, threadId: String): PendingIntent =
        PendingIntent.getActivity(
            context,
            tag(pcId, threadId).hashCode(),
            Intent(context, MainActivity::class.java)
                .setData(intentData("open", pcId, threadId, null))
                .putExtra(MainActivity.EXTRA_OPEN_AGENT_PC, pcId)
                .putExtra(MainActivity.EXTRA_OPEN_AGENT_THREAD, threadId)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )

    private fun broadcast(
        context: Context,
        action: String,
        pcId: String,
        threadId: String,
        token: String?,
        mutable: Boolean,
    ): PendingIntent {
        val intent = Intent(context, AgentActionReceiver::class.java)
            .setAction(action)
            .setData(intentData(action, pcId, threadId, token))
            .putExtra(AgentActionReceiver.EXTRA_PC, pcId)
            .putExtra(AgentActionReceiver.EXTRA_THREAD, threadId)
        if (token != null) intent.putExtra(AgentActionReceiver.EXTRA_TOKEN, token)
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or
            if (mutable && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) PendingIntent.FLAG_MUTABLE else PendingIntent.FLAG_IMMUTABLE
        return PendingIntent.getBroadcast(context, (action + tag(pcId, threadId)).hashCode(), intent, flags)
    }

    private fun intentData(kind: String, pcId: String, threadId: String, token: String?): Uri =
        Uri.Builder()
            .scheme("g7link-agent")
            .authority(pcId)
            .appendPath(threadId)
            .appendPath(kind)
            .apply { if (token != null) appendPath(token) }
            .build()

    private fun post(context: Context, tag: String, notification: Notification) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            return
        }
        createChannels(context)
        runCatching { NotificationManagerCompat.from(context).notify(tag, NOTIFICATION_ID, notification) }
    }
}
