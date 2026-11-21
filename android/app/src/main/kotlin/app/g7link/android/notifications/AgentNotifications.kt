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
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import androidx.annotation.StringRes
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import app.g7link.android.R
import app.g7link.android.ui.MainActivity
import app.g7link.android.ui.agents.AgentsModel
import app.g7link.android.ui.agents.AgentsState
import app.g7link.android.ui.agents.NoticeAnswers
import app.g7link.android.ui.agents.PcPresence
import app.g7link.android.ui.agents.ThreadStatus
import app.g7link.core.AgentAsk
import app.g7link.core.AgentOutcome
import app.g7link.core.AgentSession
import app.g7link.core.AgentSessionSource
import app.g7link.core.AgentSessionState
import app.g7link.core.Event
import java.util.Date
import java.util.concurrent.ConcurrentHashMap

/** What a thread's (or a followed session's) one notification should become. */
sealed interface AgentNotice {
    /**
     * A turn running in a thread this phone started or opened: a Live Update.
     * [step] is what it does now ("Now: Run command"); [sinceMs] when it began
     * working, from the PC; [planStep] of [planSize] when the agent made a plan.
     */
    data class Working(
        val pcId: String,
        val threadId: String,
        val agent: String,
        val folder: String,
        val title: String,
        val step: String? = null,
        val sinceMs: Long? = null,
        val planStep: Int? = null,
        val planSize: Int? = null,
    ) : AgentNotice

    /** A request waiting for an answer; [allowable] when it can be allowed without opening the app. */
    data class Waiting(
        val pcId: String,
        val threadId: String,
        val ask: AgentAsk,
        val agent: String,
        /** The folder's label, when the thread is known. */
        val folder: String?,
        val allowable: Boolean,
        /** The PC takes a reason with Deny (`agent.say_why`): "Say why" is offered. */
        val sayWhy: Boolean = false,
        /** Every request and question waiting in the thread, by token, this one included ("1 of 3"). */
        val waiting: List<String> = emptyList(),
    ) : AgentNotice

    /**
     * A question waiting for an answer. [answers] when it can be answered from
     * the notification (contract §V3.2), else it's opened in the app. [count]
     * questions wait in the thread; [text] is the agent's words when it asks one
     * thing.
     */
    data class Asking(
        val pcId: String,
        val threadId: String,
        val token: String,
        val agent: String,
        val folder: String?,
        val title: String,
        val count: Int,
        val expiresMs: Long,
        val answers: NoticeAnswers? = null,
        val text: String? = null,
        /** Every request and question waiting in the thread, by token, this one included. */
        val waiting: List<String> = emptyList(),
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
        /** The folder's label: a plan names where it is ("Claude has a plan in g7linux"). */
        val folder: String? = null,
    ) : AgentNotice

    data class Cancel(val pcId: String, val threadId: String) : AgentNotice

    /** Everything of a PC's: its session ended or nothing can run there now. */
    data class CancelPc(val pcId: String) : AgentNotice

    /** A session started on the PC that the user keeps in the status bar, working: a Live Update with Stop following. */
    data class SessionWorking(
        val pcId: String,
        val sessionId: String,
        val agent: String,
        val folder: String,
        val title: String,
        val step: String?,
        val sinceMs: Long?,
        val source: AgentSessionSource,
    ) : AgentNotice

    /** That session waits for an answer there, as the agent reported it: it's answered on the PC, so no buttons. */
    data class SessionWaiting(val pcId: String, val sessionId: String, val agent: String, val title: String) : AgentNotice

    data class CancelSession(val pcId: String, val sessionId: String) : AgentNotice

    /**
     * [notice] isn't posted while its thread is on screen on the PC and
     * someone is at it (contract §V4.4): kept in memory, and posted, alerting,
     * once that stops, if it's still true then.
     */
    data class Held(val notice: AgentNotice) : AgentNotice
}

/** How a turn ended; [PLAN]: a plan-mode turn, its plan waiting for Implement (opened in the app only). */
enum class FinishKind { DONE, FAILED, PAUSED, PLAN }

/**
 * A notification's button, and whether Android must have the phone unlocked
 * to send it. [option] is the index of a question's option ([Kind.OPTION]).
 */
data class NoticeAction(val kind: Kind, val authenticationRequired: Boolean, val option: Int? = null) {
    enum class Kind { DENY, DENY_WHY, ALLOW, REVIEW, STOP, REPLY, UNFOLLOW, OPTION, ANSWER }
}

/**
 * The rules a Live Update keeps beyond what one event says (Android's Live
 * Update guidance): a change of step updates it at most once every
 * [intervalMs] per notification, and one the user dismissed isn't posted again
 * until its turn ends. The clock is the caller's, so it can be tested.
 */
class LiveUpdateGate(private val intervalMs: Long = STEP_INTERVAL_MS) {
    sealed interface Admit {
        /** Post it now. */
        data object Now : Admit

        /** Dismissed this turn: don't post it, and take away whatever stands in its place. */
        data object Drop : Admit

        /** Updated too lately: post the newest after [delayMs]. */
        data class Later(val delayMs: Long) : Admit
    }

    private val lastPosted = HashMap<String, Long>()
    private val dismissedTags = HashSet<String>()

    @Synchronized
    fun admit(tag: String, nowMs: Long): Admit {
        if (tag in dismissedTags) return Admit.Drop
        val last = lastPosted[tag]
        if (last != null && nowMs - last in 0 until intervalMs) return Admit.Later(intervalMs - (nowMs - last))
        lastPosted[tag] = nowMs
        return Admit.Now
    }

    /** The user swiped it away: not again this turn. */
    @Synchronized
    fun dismissed(tag: String) {
        dismissedTags += tag
        lastPosted.remove(tag)
    }

    /** Something else took its place (a request): the next Live Update posts at once, unless it was dismissed. */
    @Synchronized
    fun replaced(tag: String) {
        lastPosted.remove(tag)
    }

    /** Its turn ended (done, stopped, idle, gone): a new turn may show again. */
    @Synchronized
    fun ended(tag: String) {
        dismissedTags -= tag
        lastPosted.remove(tag)
    }

    @Synchronized
    fun endedAll(prefix: String) {
        dismissedTags.removeAll { it.startsWith(prefix) }
        lastPosted.keys.removeAll { it.startsWith(prefix) }
    }

    @Synchronized
    fun isDismissed(tag: String): Boolean = tag in dismissedTags

    companion object {
        /** Android asks that a Live Update not change more often than this. */
        const val STEP_INTERVAL_MS = 3_000L
    }
}

/**
 * What the request and question notifications remember between posts
 * (contract §V3.1), in memory only: when a notification changed to a token
 * (taps on Allow, an option or Type within [guardMs] of that are ignored, as
 * they may have been meant for the one before), when the user last answered
 * from a thread's notification (the next one within [quietMs] doesn't sound),
 * and the run of tokens a thread showed since nothing waited there ("2 of 3").
 * The clock is the caller's (elapsed time), so it can be tested.
 */
class NoticeMemory(private val guardMs: Long = GUARD_MS, private val quietMs: Long = QUIET_MS) {
    /** What a request or question notification shows, and how it's posted. */
    data class Showing(
        /** It showed this token already: it doesn't sound again. */
        val again: Boolean,
        /** It replaces one the user answered from it moments ago: it doesn't sound either. */
        val quiet: Boolean,
    )

    private val shownTokens = HashMap<String, String>()
    private val changedAt = HashMap<String, Long>()
    private val answeredAt = HashMap<String, Long>()
    private val runs = HashMap<String, LinkedHashSet<String>>()

    /** [tag]'s notification is about to show [token]. */
    @Synchronized
    fun showing(tag: String, token: String, nowMs: Long): Showing {
        val before = shownTokens.put(tag, token)
        if (before == token) return Showing(again = true, quiet = false)
        changedAt.entries.removeAll { nowMs - it.value !in 0..KEEP_MS }
        changedAt[token] = nowMs
        val answered = answeredAt.remove(tag)
        val quiet = answered != null && nowMs - answered in 0..quietMs
        return Showing(again = false, quiet = quiet)
    }

    /** A tap on Allow, an option or Type for [token] is ignored: its notification changed to it moments ago. Unknown is old. */
    @Synchronized
    fun guarded(token: String, nowMs: Long): Boolean {
        val at = changedAt[token] ?: return false
        return nowMs - at in 0 until guardMs
    }

    /** The user answered from [tag]'s notification. */
    @Synchronized
    fun answered(tag: String, nowMs: Long) {
        answeredAt.entries.removeAll { nowMs - it.value !in 0..quietMs }
        answeredAt[tag] = nowMs
    }

    /**
     * Where the shown one is among [waiting] and those answered before it in
     * this run, as "position of total"; null when it's the only one. The run
     * ends when nothing waits.
     */
    @Synchronized
    fun count(tag: String, waiting: List<String>): Pair<Int, Int>? {
        if (waiting.isEmpty()) {
            runs.remove(tag)
            return null
        }
        val run = runs.getOrPut(tag) { LinkedHashSet() }
        // A run that never ends is bounded; it starts again from what waits.
        if (run.size > MAX_RUN) run.retainAll(waiting.toSet())
        val done = run.count { it !in waiting }
        run.addAll(waiting)
        val total = done + waiting.size
        return if (total > 1) (done + 1) to total else null
    }

    /** Nothing waits in [tag]'s thread any more, or its notification went. */
    @Synchronized
    fun ended(tag: String) {
        shownTokens.remove(tag)
        runs.remove(tag)
    }

    @Synchronized
    fun endedAll(prefix: String) {
        shownTokens.keys.removeAll { it.startsWith(prefix) }
        runs.keys.removeAll { it.startsWith(prefix) }
        answeredAt.keys.removeAll { it.startsWith(prefix) }
    }

    companion object {
        /** The same as the dock's typing guard. */
        const val GUARD_MS = 1_500L
        const val QUIET_MS = 10_000L
        private const val KEEP_MS = 60_000L
        private const val MAX_RUN = 64
    }
}

/**
 * Notices kept back while their thread is on screen on the PC (contract
 * §V4.4), by tag, in memory only: after a restart the first plan posts
 * whatever is current.
 */
class HeldNotices {
    private val held = LinkedHashMap<String, AgentNotice>()

    @Synchronized
    fun hold(tag: String, notice: AgentNotice) {
        held[tag] = notice
    }

    @Synchronized
    fun drop(tag: String) {
        held.remove(tag)
    }

    @Synchronized
    fun dropAll(prefix: String) {
        held.keys.removeAll { it.startsWith(prefix) }
    }

    @Synchronized
    fun has(tag: String): Boolean = tag in held

    /** The notices under [prefix] that no longer [holds], taken out to be posted if they're still true. */
    @Synchronized
    fun release(prefix: String, holds: (AgentNotice) -> Boolean): List<AgentNotice> {
        val out = held.filter { (tag, notice) -> tag.startsWith(prefix) && !holds(notice) }
        out.keys.forEach(held::remove)
        return out.values.toList()
    }
}

/**
 * A PC's agent threads on this phone's notifications (docs/protocol/agents.md):
 * one per thread, replaced in place, by tag `agent:<pc>:<thread>`, and one per
 * followed session kept in the status bar, by tag `agent-session:<pc>:<session>`.
 * A request waiting for an answer hides its command on the lock screen; Allow
 * and Reply need the phone unlocked, Deny and Stop don't. A working thread is
 * a Live Update (promoted on Android 16) that never asks: a request replaces it
 * with a separate high-importance notification.
 */
object AgentNotifications {
    private const val CHANNEL_WORKING = "agents_working"
    private const val CHANNEL_WAITING = "agents_waiting"
    private const val CHANNEL_DONE = "agents_done"
    private const val NOTIFICATION_ID = 0x4147_4E54 // "AGNT"
    private const val TAG_PREFIX = "agent:"
    private const val SESSION_TAG_PREFIX = "agent-session:"
    private const val LIMIT_TAG_PREFIX = "agent-limit:"

    /** The reply typed under a finished thread's notification. */
    const val REPLY_KEY = "agent_reply"

    /** The reason typed under "Say why" on a request's notification. */
    const val DENY_WHY_KEY = "agent_deny_why"

    /** The answer typed under "Type" on a question's notification. */
    const val ANSWER_KEY = "agent_answer"

    /** Android lets an action require an unlocked phone from 12 (S) on; earlier, Allow and Reply aren't offered. */
    private const val AUTH_SDK = AgentsModel.NOTICE_AUTH_SDK

    /** How much of a command or change shows in the expanded notification. */
    private const val MAX_BODY_CHARS = 600

    /** When each thread was first seen working, for its "working for" timer when the PC doesn't say. */
    private val workingSince = ConcurrentHashMap<String, Long>()

    /** The request each notification last showed, so a repost of the same one doesn't sound again. */
    private val shownAsk = ConcurrentHashMap<String, String>()

    /** Step changes paced and dismissed Live Updates kept away. */
    val gate = LiveUpdateGate()

    /** The newest Live Update waiting for its turn under [gate], by tag. */
    private val pending = ConcurrentHashMap<String, AgentNotice>()
    private val main by lazy { Handler(Looper.getMainLooper()) }

    /** A session's needs-you notice already sounded for this wait. */
    private const val SESSION_WAITING_MARK = "session-waiting"

    /** This phone's Android version; 0 where there's none to read (JVM tests). */
    private val deviceSdk: Int by lazy { runCatching { Build.VERSION.SDK_INT }.getOrDefault(0) }

    /** The tap guard, the quiet replace and the count of requests and questions. */
    val memory = NoticeMemory()

    /** Notices kept back while their thread is on screen on the PC. */
    val held = HeldNotices()

    fun tag(pcId: String, threadId: String): String = "$TAG_PREFIX$pcId:$threadId"

    fun sessionTag(pcId: String, sessionId: String): String = "$SESSION_TAG_PREFIX$pcId:$sessionId"

    fun createChannels(context: Context) {
        val manager = context.getSystemService(NotificationManager::class.java) ?: return
        manager.createNotificationChannel(
            NotificationChannel(
                CHANNEL_WORKING,
                context.getString(R.string.channel_agents_working),
                // Low, not min: Android promotes Live Updates only from a channel above min.
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
     * (nothing posts for it); [shownSession] the followed session on screen;
     * [kept] whether the user keeps a session in the status bar; [followed]
     * whether this phone started or opened a thread (only those show while
     * they work).
     */
    fun plan(
        pcId: String,
        event: Event,
        state: AgentsState,
        shown: String?,
        shownSession: String? = null,
        kept: (String) -> Boolean = { false },
        presence: PcPresence? = null,
        quietAtPc: Boolean = false,
        nowMs: Long = System.currentTimeMillis(),
        sdkInt: Int = deviceSdk,
        followed: (String) -> Boolean,
    ): List<AgentNotice> = planned(pcId, event, state, shown, shownSession, kept, sdkInt, followed).map { held(it, presence, quietAtPc, nowMs) }

    /**
     * [notice] as [AgentNotice.Held] while the PC keeps it back (contract
     * §V4.4): a request, a question or a finished turn, the setting on, and
     * someone at the PC looking at that thread. A Live Update never waits.
     */
    fun held(notice: AgentNotice, presence: PcPresence?, quietAtPc: Boolean, nowMs: Long): AgentNotice {
        if (!quietAtPc || presence == null) return notice
        val threadId = when (notice) {
            is AgentNotice.Waiting -> notice.threadId
            is AgentNotice.Asking -> notice.threadId
            is AgentNotice.Finished -> notice.threadId
            else -> return notice
        }
        return if (presence.quiets(threadId, nowMs)) AgentNotice.Held(notice) else notice
    }

    /**
     * What a notice kept back becomes once it no longer waits: a request or
     * question, whatever the thread shows now (null when nothing waits there);
     * a finished turn, itself while the thread is still unread (one seen
     * finishing on the PC is read already).
     */
    fun released(
        pcId: String,
        notice: AgentNotice,
        state: AgentsState,
        shown: String?,
        sdkInt: Int = deviceSdk,
        followed: (String) -> Boolean,
    ): AgentNotice? = when (notice) {
        is AgentNotice.Waiting -> threadNotice(pcId, state, notice.threadId, shown, sdkInt, followed)?.takeIf { it.waits() }
        is AgentNotice.Asking -> threadNotice(pcId, state, notice.threadId, shown, sdkInt, followed)?.takeIf { it.waits() }
        is AgentNotice.Finished -> notice.takeIf { notice.threadId != shown && state.thread(notice.threadId)?.unread == true }
        else -> null
    }

    private fun AgentNotice.waits(): Boolean = this is AgentNotice.Waiting || this is AgentNotice.Asking

    private fun planned(
        pcId: String,
        event: Event,
        state: AgentsState,
        shown: String?,
        shownSession: String?,
        kept: (String) -> Boolean,
        sdkInt: Int,
        followed: (String) -> Boolean,
    ): List<AgentNotice> = when (event) {
        is Event.AgentCatalog -> if (event.catalog == null) listOf(AgentNotice.CancelPc(pcId)) else emptyList()
        is Event.AgentThreadRemoved -> listOf(AgentNotice.Cancel(pcId, event.thread))
        is Event.AgentAsk -> listOfNotNull(threadNotice(pcId, state, event.ask.thread, shown, sdkInt, followed))
        is Event.AgentAsked -> listOfNotNull(threadNotice(pcId, state, event.thread, shown, sdkInt, followed))
        is Event.AgentQuestion -> listOfNotNull(threadNotice(pcId, state, event.question.thread, shown, sdkInt, followed))
        is Event.AgentQuestioned -> listOfNotNull(threadNotice(pcId, state, event.thread, shown, sdkInt, followed))
        // A turn's end is AgentDone's; a change while it runs can bring the working one back.
        is Event.AgentThreadChanged ->
            if (AgentsModel.running(event.thread)) listOfNotNull(threadNotice(pcId, state, event.thread.id, shown, sdkInt, followed)) else emptyList()
        is Event.AgentDone -> listOf(finished(pcId, state, event, shown))
        is Event.AgentSessionChanged -> listOfNotNull(sessionNotice(pcId, state, event.session, shownSession, kept))
        is Event.AgentSessionRemoved -> listOf(AgentNotice.CancelSession(pcId, event.session))
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
        sdkInt: Int = deviceSdk,
        followed: (String) -> Boolean,
    ): AgentNotice? {
        if (threadId == shown) return null
        val thread = state.thread(threadId)
        val asks = state.asksFor(threadId)
        val questions = state.questionsFor(threadId)
        val waiting = asks.map { it.token } + questions.map { it.token }
        val ask = asks.firstOrNull()
        if (ask != null) {
            return AgentNotice.Waiting(
                pcId = pcId,
                threadId = threadId,
                ask = ask,
                agent = state.agentName(thread?.agent ?: ask.agent),
                folder = thread?.folder?.takeIf { it.isNotBlank() },
                allowable = AgentsModel.allowableInNotification(ask),
                sayWhy = state.catalog?.canSayWhy == true,
                waiting = waiting,
            )
        }
        val question = questions.firstOrNull()
        if (question != null) {
            return AgentNotice.Asking(
                pcId = pcId,
                threadId = threadId,
                token = question.token,
                agent = state.agentName(thread?.agent ?: question.agent),
                folder = thread?.folder?.takeIf { it.isNotBlank() },
                title = thread?.title.orEmpty(),
                count = questions.size,
                expiresMs = question.expiresMs,
                answers = AgentsModel.answerableInNotification(question, sdkInt),
                text = question.asks.singleOrNull()?.text?.takeIf { !question.secret && it.isNotBlank() },
                waiting = waiting,
            )
        }
        if (thread == null) return AgentNotice.Cancel(pcId, threadId)
        if (!AgentsModel.running(thread)) return null
        return if (followed(threadId)) {
            val plan = thread.plan
            AgentNotice.Working(
                pcId = pcId,
                threadId = threadId,
                agent = state.agentName(thread.agent),
                folder = thread.folder,
                title = thread.title,
                step = thread.step?.takeIf { it.isNotBlank() },
                sinceMs = thread.sinceMs?.takeIf { it > 0L },
                planStep = AgentsModel.planPosition(plan),
                planSize = plan?.size?.takeIf { it > 0 },
            )
        } else {
            AgentNotice.Cancel(pcId, threadId)
        }
    }

    /**
     * What a session's notification should show now. Only a session the user
     * keeps in the status bar has one: a Live Update while it works (a guessed
     * "may need you" still reads as working), a needs-you notice with nothing to
     * press while it waits as the agent reported, and none while idle or ended.
     */
    fun sessionNotice(
        pcId: String,
        state: AgentsState,
        session: AgentSession,
        shownSession: String?,
        kept: (String) -> Boolean,
    ): AgentNotice? {
        if (!kept(session.id)) return null
        if (session.id == shownSession) return AgentNotice.CancelSession(pcId, session.id)
        val agent = state.agentName(session.agent)
        return when (session.state) {
            AgentSessionState.NEEDS_YOU, AgentSessionState.MAY_NEED_YOU, AgentSessionState.WORKING ->
                if (AgentsModel.needsYouExactly(session)) {
                    AgentNotice.SessionWaiting(pcId, session.id, agent, session.title)
                } else {
                    AgentNotice.SessionWorking(
                        pcId = pcId,
                        sessionId = session.id,
                        agent = agent,
                        folder = session.folder,
                        title = session.title,
                        step = session.step?.takeIf { it.isNotBlank() },
                        sinceMs = session.sinceMs?.takeIf { it > 0L },
                        source = session.source,
                    )
                }
            AgentSessionState.IDLE, AgentSessionState.ENDED -> AgentNotice.CancelSession(pcId, session.id)
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
                if (AgentsModel.status(thread) == ThreadStatus.PlanReady) FinishKind.PLAN else FinishKind.DONE,
                event.summary.ifBlank { thread.summary.orEmpty() }.ifBlank { null },
                folder = thread.folder.ifBlank { null },
            )
            AgentOutcome.FAILED -> when (val status = AgentsModel.failure(thread.error)) {
                is ThreadStatus.Paused -> AgentNotice.Finished(pcId, thread.id, agent, thread.title, FinishKind.PAUSED, null, status.resetsMs)
                else -> AgentNotice.Finished(pcId, thread.id, agent, thread.title, FinishKind.FAILED, AgentsModel.shownError(thread.error))
            }
        }
    }

    /**
     * A waiting request's buttons (contract §V3.1): Deny first, then Say why
     * (a reason typed in place, sent with Deny, while the PC takes one), then
     * Allow or Review. Allow and Say why need the phone unlocked; Deny
     * doesn't, as it can only stop something.
     */
    fun waitingActions(allowable: Boolean, sdkInt: Int, sayWhy: Boolean = false): List<NoticeAction> = listOfNotNull(
        NoticeAction(NoticeAction.Kind.DENY, authenticationRequired = false),
        if (sayWhy && sdkInt >= AUTH_SDK) NoticeAction(NoticeAction.Kind.DENY_WHY, authenticationRequired = true) else null,
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

    /**
     * A question's buttons: its options in order and Type when it fits a
     * notification (contract §V3.2), each needing the phone unlocked; else
     * one button that opens the thread.
     */
    fun askingActions(answers: NoticeAnswers? = null): List<NoticeAction> {
        if (answers == null) return listOf(NoticeAction(NoticeAction.Kind.REVIEW, authenticationRequired = false))
        val options = answers.options.indices.map { NoticeAction(NoticeAction.Kind.OPTION, authenticationRequired = true, option = it) }
        return if (answers.type) options + NoticeAction(NoticeAction.Kind.ANSWER, authenticationRequired = true) else options
    }

    /** A question on the lock screen: who asks and where, opened in the app. */
    fun publicAskingActions(): List<NoticeAction> = emptyList()

    /** The buttons the tap guard covers: those that let something happen. Deny and Stop only ever stop it. */
    fun guards(kind: NoticeAction.Kind): Boolean =
        kind == NoticeAction.Kind.ALLOW || kind == NoticeAction.Kind.OPTION || kind == NoticeAction.Kind.ANSWER

    /** Whether a tap on Allow, an option or Type for [token] is ignored: its notification changed to it under 1.5 s ago. */
    fun guarded(token: String): Boolean = memory.guarded(token, SystemClock.elapsedRealtime())

    /** The user answered from the thread's notification: the next one within 10 s doesn't sound. */
    fun answered(pcId: String, threadId: String) = memory.answered(tag(pcId, threadId), SystemClock.elapsedRealtime())

    /** A followed session can't be stopped from here, only no longer kept in the status bar. */
    fun sessionWorkingActions(): List<NoticeAction> = listOf(NoticeAction(NoticeAction.Kind.UNFOLLOW, authenticationRequired = false))

    /** A session waiting in a terminal is answered there: nothing to press here. */
    fun sessionWaitingActions(): List<NoticeAction> = emptyList()

    /** A finished turn can be replied to from the notification, unlocked; a paused one can't yet, a plan is opened in the app. */
    fun finishedActions(kind: FinishKind, sdkInt: Int): List<NoticeAction> =
        if (kind != FinishKind.PAUSED && kind != FinishKind.PLAN && sdkInt >= AUTH_SDK) {
            listOf(NoticeAction(NoticeAction.Kind.REPLY, authenticationRequired = true))
        } else {
            emptyList()
        }

    /**
     * The lines a working thread's Live Update shows under its title: the
     * thread's title, what it does now, and where it is in its plan when it made
     * one. [now] and [stepOf] are the app's own strings.
     */
    fun workingLines(notice: AgentNotice.Working, now: (String) -> String, stepOf: (Int, Int) -> String): List<String> =
        listOfNotNull(
            notice.title.ifBlank { null },
            notice.step?.let(now),
            if (notice.planStep != null && notice.planSize != null) stepOf(notice.planStep, notice.planSize) else null,
        )

    /** Posts and cancels what [plan] or [threadNotice] decided. */
    fun apply(context: Context, pcName: String, notices: List<AgentNotice>) {
        for (notice in notices) {
            // Anything newer for a thread replaces what was kept back for it.
            if (notice !is AgentNotice.Held) tagOf(notice)?.let(held::drop)
            when (notice) {
                is AgentNotice.Held -> hold(context, notice.notice)
                is AgentNotice.Cancel -> cancel(context, notice.pcId, notice.threadId)
                is AgentNotice.CancelPc -> cancelPc(context, notice.pcId)
                is AgentNotice.Working -> live(context, pcName, tag(notice.pcId, notice.threadId), notice)
                is AgentNotice.Waiting -> {
                    val tag = tag(notice.pcId, notice.threadId)
                    pending.remove(tag)
                    gate.replaced(tag)
                    shownAsk[tag] = notice.ask.token
                    val showing = memory.showing(tag, notice.ask.token, SystemClock.elapsedRealtime())
                    val count = memory.count(tag, notice.waiting)
                    post(context, tag, waiting(context, pcName, notice, quiet = showing.again || showing.quiet, count))
                }
                is AgentNotice.Asking -> {
                    val tag = tag(notice.pcId, notice.threadId)
                    pending.remove(tag)
                    gate.replaced(tag)
                    shownAsk[tag] = notice.token
                    val showing = memory.showing(tag, notice.token, SystemClock.elapsedRealtime())
                    val count = memory.count(tag, notice.waiting)
                    post(context, tag, asking(context, pcName, notice, quiet = showing.again || showing.quiet, count))
                }
                is AgentNotice.Finished -> {
                    val tag = tag(notice.pcId, notice.threadId)
                    pending.remove(tag)
                    gate.ended(tag)
                    workingSince.remove(tag)
                    shownAsk.remove(tag)
                    memory.ended(tag)
                    post(context, tag, finished(context, pcName, notice))
                }
                is AgentNotice.SessionWorking -> live(context, pcName, sessionTag(notice.pcId, notice.sessionId), notice)
                is AgentNotice.SessionWaiting -> {
                    val tag = sessionTag(notice.pcId, notice.sessionId)
                    pending.remove(tag)
                    gate.replaced(tag)
                    val again = shownAsk[tag] == SESSION_WAITING_MARK
                    shownAsk[tag] = SESSION_WAITING_MARK
                    post(context, tag, sessionWaiting(context, pcName, notice, again))
                }
                is AgentNotice.CancelSession -> cancelSession(context, notice.pcId, notice.sessionId)
            }
        }
    }

    /** The tag of the notification [notice] is about; null for a whole PC's. */
    private fun tagOf(notice: AgentNotice): String? = when (notice) {
        is AgentNotice.Working -> tag(notice.pcId, notice.threadId)
        is AgentNotice.Waiting -> tag(notice.pcId, notice.threadId)
        is AgentNotice.Asking -> tag(notice.pcId, notice.threadId)
        is AgentNotice.Finished -> tag(notice.pcId, notice.threadId)
        is AgentNotice.Cancel -> tag(notice.pcId, notice.threadId)
        is AgentNotice.Held -> tagOf(notice.notice)
        is AgentNotice.SessionWorking, is AgentNotice.SessionWaiting, is AgentNotice.CancelSession, is AgentNotice.CancelPc -> null
    }

    /**
     * Keeps [notice] back while its thread is on screen on the PC: whatever
     * this thread's notification showed goes, so nothing stale can be
     * answered from it, and it's posted, alerting, once it's let go.
     */
    private fun hold(context: Context, notice: AgentNotice) {
        val tag = tagOf(notice) ?: return
        pending.remove(tag)
        gate.replaced(tag)
        shownAsk.remove(tag)
        memory.ended(tag)
        held.hold(tag, notice)
        runCatching { NotificationManagerCompat.from(context).cancel(tag, NOTIFICATION_ID) }
    }

    /** The notices of [pcId] kept back that no longer wait, taken out ([released] says what each becomes). */
    fun releasable(pcId: String, holds: (AgentNotice) -> Boolean): List<AgentNotice> = held.release("$TAG_PREFIX$pcId:", holds)

    /**
     * Shows again, quietly, what a request's or question's notification
     * showed: a tap that was ignored, or a reason or answer left empty. Typed
     * text Android was sending stops spinning; nothing else changes.
     */
    fun restore(context: Context, pcId: String, threadId: String) {
        val tag = tag(pcId, threadId)
        val manager = context.getSystemService(NotificationManager::class.java) ?: return
        val shown = runCatching { manager.activeNotifications.firstOrNull { it.id == NOTIFICATION_ID && it.tag == tag } }.getOrNull() ?: return
        val again = runCatching {
            Notification.Builder.recoverBuilder(context, shown.notification).setOnlyAlertOnce(true).build()
        }.getOrNull() ?: return
        post(context, tag, again)
    }

    /** A Live Update, through [gate]: paced, and kept away once dismissed this turn. */
    private fun live(context: Context, pcName: String, tag: String, notice: AgentNotice) {
        // Working again after a request: the request's notice is done with.
        if (shownAsk[tag] != null) {
            shownAsk.remove(tag)
            gate.replaced(tag)
        }
        memory.ended(tag)
        when (val admit = gate.admit(tag, SystemClock.elapsedRealtime())) {
            LiveUpdateGate.Admit.Now -> {
                pending.remove(tag)
                postLive(context, pcName, tag, notice)
            }
            LiveUpdateGate.Admit.Drop -> {
                pending.remove(tag)
                runCatching { NotificationManagerCompat.from(context).cancel(tag, NOTIFICATION_ID) }
            }
            is LiveUpdateGate.Admit.Later -> {
                val first = pending.put(tag, notice) == null
                if (first) {
                    val app = context.applicationContext
                    main.postDelayed({
                        val latest = pending.remove(tag) ?: return@postDelayed
                        live(app, pcName, tag, latest)
                    }, admit.delayMs)
                }
            }
        }
    }

    private fun postLive(context: Context, pcName: String, tag: String, notice: AgentNotice) {
        when (notice) {
            is AgentNotice.Working -> post(context, tag, working(context, pcName, tag, notice))
            is AgentNotice.SessionWorking -> post(context, tag, sessionWorking(context, pcName, tag, notice))
            else -> Unit
        }
    }

    /** The user swiped a Live Update away (its delete intent): not again this turn. */
    fun dismissed(tag: String) {
        pending.remove(tag)
        gate.dismissed(tag)
    }

    fun cancel(context: Context, pcId: String, threadId: String) {
        val tag = tag(pcId, threadId)
        pending.remove(tag)
        gate.ended(tag)
        workingSince.remove(tag)
        shownAsk.remove(tag)
        memory.ended(tag)
        held.drop(tag)
        runCatching { NotificationManagerCompat.from(context).cancel(tag, NOTIFICATION_ID) }
    }

    fun cancelSession(context: Context, pcId: String, sessionId: String) {
        val tag = sessionTag(pcId, sessionId)
        pending.remove(tag)
        gate.ended(tag)
        workingSince.remove(tag)
        shownAsk.remove(tag)
        runCatching { NotificationManagerCompat.from(context).cancel(tag, NOTIFICATION_ID) }
    }

    fun cancelPc(context: Context, pcId: String) {
        val prefixes = listOf("$TAG_PREFIX$pcId:", "$SESSION_TAG_PREFIX$pcId:")
        for (prefix in prefixes) {
            workingSince.keys.removeAll { it.startsWith(prefix) }
            shownAsk.keys.removeAll { it.startsWith(prefix) }
            pending.keys.removeAll { it.startsWith(prefix) }
            gate.endedAll(prefix)
            memory.endedAll(prefix)
            held.dropAll(prefix)
        }
        val manager = context.getSystemService(NotificationManager::class.java) ?: return
        runCatching {
            manager.activeNotifications
                .filter { notification -> notification.id == NOTIFICATION_ID && prefixes.any { notification.tag?.startsWith(it) == true } }
                .forEach { manager.cancel(it.tag, NOTIFICATION_ID) }
        }
    }

    /**
     * "Claude's limit has reset" (contract §V5), by tag `agent-limit:<pc>:<agent>`.
     * Open goes to the newest thread the limit paused with the reply box
     * focused and nothing typed, or to the list; nothing here sends.
     */
    fun limitReset(context: Context, alarm: LimitAlarm) {
        val tag = "$LIMIT_TAG_PREFIX${alarm.pcId}:${alarm.agent}"
        val title = context.getString(R.string.agents_limit_reset_title, alarm.agentName)
        val text = alarm.threadTitle?.let { context.getString(R.string.agents_limit_reset_text, it) }
        val intent = Intent(context, MainActivity::class.java)
            .setData(Uri.Builder().scheme("g7link-agent-limit").authority(alarm.pcId).appendPath(alarm.agent).build())
            .putExtra(MainActivity.EXTRA_OPEN_AGENT_PC, alarm.pcId)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        if (alarm.threadId != null) {
            intent.putExtra(MainActivity.EXTRA_OPEN_AGENT_THREAD, alarm.threadId)
            intent.putExtra(MainActivity.EXTRA_FOCUS_AGENT_REPLY, true)
        }
        val open = PendingIntent.getActivity(context, tag.hashCode(), intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        // On the lock screen: whose limit, never the thread.
        val public = Notification.Builder(context, CHANNEL_DONE)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(title)
            .setContentIntent(open)
        val builder = Notification.Builder(context, CHANNEL_DONE)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(title)
            .setContentIntent(open)
            .setVisibility(Notification.VISIBILITY_PRIVATE)
            .setPublicVersion(public.build())
            .setAutoCancel(true)
            .setShowWhen(true)
            .setWhen(System.currentTimeMillis())
            .setCategory(Notification.CATEGORY_REMINDER)
        if (alarm.pcName.isNotBlank()) builder.setSubText(alarm.pcName)
        if (text != null) builder.setContentText(text)
        val icon = Icon.createWithResource(context, R.drawable.ic_notification)
        builder.addAction(Notification.Action.Builder(icon, context.getString(R.string.agents_link_open), open).build())
        post(context, tag, builder.build())
    }

    /** The PC was unpaired, or its limit alarm turned off: what its alarms posted goes. */
    fun cancelLimitResets(context: Context, pcId: String) {
        val prefix = "$LIMIT_TAG_PREFIX$pcId:"
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
        pending.remove(tag)
        workingSince.remove(tag)
        shownAsk.remove(tag)
        memory.ended(tag)
        held.drop(tag)
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
    private fun working(context: Context, pcName: String, tag: String, notice: AgentNotice.Working): Notification {
        val since = notice.sinceMs ?: workingSince.getOrPut(tag) { System.currentTimeMillis() }
        val lines = workingLines(
            notice,
            now = { context.getString(R.string.agents_step_now, it) },
            stepOf = { at, of -> context.getString(R.string.agents_step_of, at, of) },
        )
        val open = openIntent(context, notice.pcId, notice.threadId)
        // On the lock screen: who works on which PC, never the folder, the step or the plan.
        val public = Notification.Builder(context, CHANNEL_WORKING)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(context.getString(R.string.agents_notify_working_public, notice.agent, pcName))
            .setContentIntent(open)
            .setShowWhen(true)
            .setWhen(since)
            .setUsesChronometer(true)
        for (action in workingActions()) public.addAction(action(context, action, notice.pcId, notice.threadId, null))
        val builder = Notification.Builder(context, CHANNEL_WORKING)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(context.getString(R.string.agents_notify_working, notice.agent, notice.folder))
            .setContentText(lines.firstOrNull() ?: notice.title)
            .setStyle(Notification.BigTextStyle().bigText(lines.joinToString("\n")))
            .setSubText(pcName)
            .setContentIntent(open)
            .setDeleteIntent(dismissIntent(context, tag, notice.pcId, thread = notice.threadId, session = null))
            .setVisibility(Notification.VISIBILITY_PRIVATE)
            .setPublicVersion(public.build())
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setShowWhen(true)
            .setWhen(since)
            .setUsesChronometer(true)
            .setCategory(Notification.CATEGORY_PROGRESS)
        for (action in workingActions()) builder.addAction(action(context, action, notice.pcId, notice.threadId, null))
        promote(builder, notice.agent)
        return builder.build()
    }

    @SuppressLint("NewApi")
    private fun sessionWorking(context: Context, pcName: String, tag: String, notice: AgentNotice.SessionWorking): Notification {
        val since = notice.sinceMs ?: workingSince.getOrPut(tag) { System.currentTimeMillis() }
        val where = startedWhere(context, notice.source, notice.agent, pcName)
        val lines = listOfNotNull(
            notice.title.ifBlank { null },
            notice.step?.let { context.getString(R.string.agents_step_now, it) },
            where,
        )
        val open = openSessionIntent(context, notice.pcId, notice.sessionId)
        val public = Notification.Builder(context, CHANNEL_WORKING)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(context.getString(R.string.agents_notify_working_public, notice.agent, pcName))
            .setContentIntent(open)
            .setShowWhen(true)
            .setWhen(since)
            .setUsesChronometer(true)
        for (action in sessionWorkingActions()) public.addAction(sessionAction(context, action, notice.pcId, notice.sessionId))
        val builder = Notification.Builder(context, CHANNEL_WORKING)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(context.getString(R.string.agents_notify_working, notice.agent, notice.folder))
            .setContentText(lines.firstOrNull() ?: where)
            .setStyle(Notification.BigTextStyle().bigText(lines.joinToString("\n")))
            .setSubText(pcName)
            .setContentIntent(open)
            .setDeleteIntent(dismissIntent(context, tag, notice.pcId, thread = null, session = notice.sessionId))
            .setVisibility(Notification.VISIBILITY_PRIVATE)
            .setPublicVersion(public.build())
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setShowWhen(true)
            .setWhen(since)
            .setUsesChronometer(true)
            .setCategory(Notification.CATEGORY_PROGRESS)
        for (action in sessionWorkingActions()) builder.addAction(sessionAction(context, action, notice.pcId, notice.sessionId))
        promote(builder, notice.agent)
        return builder.build()
    }

    /**
     * Asks Android 16 to show it as a Live Update: in the status bar's chip
     * (the agent's name, which fits the chip) and at the top of the shade. The
     * standard and big-text styles both qualify; no bar, not colorized.
     */
    @SuppressLint("NewApi")
    private fun promote(builder: Notification.Builder, agent: String) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.BAKLAVA) {
            runCatching { builder.setRequestPromotedOngoing(true) }
            runCatching { builder.setShortCriticalText(agent) }
        }
    }

    private fun startedWhere(context: Context, source: AgentSessionSource, agent: String, pcName: String): String = when (source) {
        AgentSessionSource.TERMINAL -> context.getString(R.string.agents_session_started_terminal, pcName)
        AgentSessionSource.DESKTOP_APP -> context.getString(R.string.agents_session_started_app, agent, pcName)
        AgentSessionSource.OTHER -> context.getString(R.string.agents_session_started_other, pcName)
    }

    /** "Kilele · 1 of 3": the PC, and where this one is among those waiting in the thread. */
    private fun subText(context: Context, pcName: String, count: Pair<Int, Int>?): String =
        if (count == null) pcName else listOf(pcName, context.getString(R.string.agents_notify_count, count.first, count.second)).joinToString(" · ")

    private fun waiting(context: Context, pcName: String, notice: AgentNotice.Waiting, quiet: Boolean, count: Pair<Int, Int>?): Notification {
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
            .setSubText(subText(context, pcName, count))
            .setContentIntent(open)
            .setCategory(Notification.CATEGORY_MESSAGE)
            .setVisibility(Notification.VISIBILITY_PRIVATE)
            .setPublicVersion(public.build())
            .setOnlyAlertOnce(quiet)
            .setShowWhen(true)
            .setWhen(now)
        // The request lapses on the PC after its time; so does its notification.
        if (ask.expiresMs > now) builder.setTimeoutAfter(ask.expiresMs - now)
        for (action in waitingActions(notice.allowable, Build.VERSION.SDK_INT, notice.sayWhy)) {
            builder.addAction(action(context, action, notice.pcId, notice.threadId, ask.token))
        }
        return builder.build()
    }

    /**
     * A question waits: who asks and where, and what when it asks one thing;
     * on the lock screen, only who. Its options and Type when it fits here
     * (contract §V3.2), else "Open to answer" and Review.
     */
    private fun asking(context: Context, pcName: String, notice: AgentNotice.Asking, quiet: Boolean, count: Pair<Int, Int>?): Notification {
        val open = openIntent(context, notice.pcId, notice.threadId)
        val publicTitle = context.getString(R.string.agents_notify_waiting_public, notice.agent, pcName)
        val public = Notification.Builder(context, CHANNEL_WAITING)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(publicTitle)
            .setContentIntent(open)
            .setCategory(Notification.CATEGORY_MESSAGE)
        val title = if (notice.folder != null) {
            context.getString(R.string.agents_notify_waiting_title, notice.agent, notice.folder)
        } else {
            publicTitle
        }
        val asks = context.resources.getQuantityString(R.plurals.agents_question_asked, notice.count, notice.count)
        val words = notice.text?.take(MAX_BODY_CHARS)
        val answers = notice.answers
        val lines = listOfNotNull(
            notice.title.ifBlank { null },
            words ?: asks,
            if (answers == null) context.getString(R.string.agents_notify_open_to_answer) else null,
        )
        val now = System.currentTimeMillis()
        val builder = Notification.Builder(context, CHANNEL_WAITING)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(title)
            .setContentText(words ?: asks)
            .setStyle(Notification.BigTextStyle().bigText(lines.joinToString("\n")))
            .setSubText(subText(context, pcName, count))
            .setContentIntent(open)
            .setCategory(Notification.CATEGORY_MESSAGE)
            .setVisibility(Notification.VISIBILITY_PRIVATE)
            .setPublicVersion(public.build())
            .setOnlyAlertOnce(quiet)
            .setShowWhen(true)
            .setWhen(now)
        if (notice.expiresMs > now) builder.setTimeoutAfter(notice.expiresMs - now)
        for (action in askingActions(answers)) {
            builder.addAction(action(context, action, notice.pcId, notice.threadId, notice.token, answers))
        }
        return builder.build()
    }

    /** A kept session waits in its terminal or app on the PC: answered there, so nothing to press. */
    private fun sessionWaiting(context: Context, pcName: String, notice: AgentNotice.SessionWaiting, again: Boolean): Notification {
        val title = context.getString(R.string.agents_notify_waiting_public, notice.agent, pcName)
        val answer = context.getString(R.string.agents_session_answer_on, pcName)
        val open = openSessionIntent(context, notice.pcId, notice.sessionId)
        val public = Notification.Builder(context, CHANNEL_WAITING)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(title)
            .setContentText(answer)
            .setContentIntent(open)
            .setCategory(Notification.CATEGORY_MESSAGE)
        val builder = Notification.Builder(context, CHANNEL_WAITING)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(title)
            .setContentText(answer)
            .setStyle(Notification.BigTextStyle().bigText(listOfNotNull(notice.title.ifBlank { null }, answer).joinToString("\n")))
            .setSubText(pcName)
            .setContentIntent(open)
            .setCategory(Notification.CATEGORY_MESSAGE)
            .setVisibility(Notification.VISIBILITY_PRIVATE)
            .setPublicVersion(public.build())
            .setOnlyAlertOnce(again)
            .setShowWhen(true)
            .setWhen(System.currentTimeMillis())
        for (action in sessionWaitingActions()) builder.addAction(sessionAction(context, action, notice.pcId, notice.sessionId))
        return builder.build()
    }

    private fun finished(context: Context, pcName: String, notice: AgentNotice.Finished): Notification {
        val title = when (notice.kind) {
            FinishKind.DONE -> context.getString(R.string.agents_notify_done_title, notice.agent)
            FinishKind.FAILED -> context.getString(R.string.agents_notify_failed_title, notice.agent)
            FinishKind.PAUSED -> context.getString(R.string.agents_notify_paused_title, notice.agent)
            FinishKind.PLAN -> notice.folder?.let { context.getString(R.string.agents_notify_plan_title, notice.agent, it) }
                ?: context.getString(R.string.agents_notify_done_title, notice.agent)
        }
        val detail = when (notice.kind) {
            FinishKind.PAUSED -> notice.resetsMs?.let {
                context.getString(R.string.agents_usage_resets, android.text.format.DateFormat.getTimeFormat(context).format(Date(it)))
            } ?: context.getString(R.string.agents_usage_limit)
            else -> notice.text
        }
        val expanded = listOfNotNull(notice.title.ifBlank { null }, detail?.ifBlank { null }).joinToString("\n")
        val open = openIntent(context, notice.pcId, notice.threadId)
        // On the lock screen: who finished, never the thread, its folder, its summary or the error.
        val public = Notification.Builder(context, CHANNEL_DONE)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(if (notice.kind == FinishKind.PLAN) context.getString(R.string.agents_notify_done_title, notice.agent) else title)
            .setContentIntent(open)
        val builder = Notification.Builder(context, CHANNEL_DONE)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(title)
            .setContentText(notice.title)
            .setStyle(Notification.BigTextStyle().bigText(expanded))
            .setSubText(pcName)
            .setContentIntent(open)
            .setVisibility(Notification.VISIBILITY_PRIVATE)
            .setPublicVersion(public.build())
            .setAutoCancel(true)
            .setShowWhen(true)
            .setWhen(System.currentTimeMillis())
            .setCategory(if (notice.kind == FinishKind.DONE || notice.kind == FinishKind.PLAN) Notification.CATEGORY_STATUS else Notification.CATEGORY_ERROR)
        for (action in finishedActions(notice.kind, Build.VERSION.SDK_INT)) {
            builder.addAction(action(context, action, notice.pcId, notice.threadId, null))
        }
        return builder.build()
    }

    @SuppressLint("NewApi")
    private fun action(
        context: Context,
        spec: NoticeAction,
        pcId: String,
        threadId: String,
        token: String?,
        answers: NoticeAnswers? = null,
    ): Notification.Action {
        val icon = Icon.createWithResource(context, R.drawable.ic_notification)
        val (label, intent) = when (spec.kind) {
            NoticeAction.Kind.REVIEW -> context.getString(R.string.agents_review) to openIntent(context, pcId, threadId)
            NoticeAction.Kind.DENY -> context.getString(R.string.agents_deny) to
                broadcast(context, AgentActionReceiver.ACTION_DENY, pcId, threadId, token, mutable = false)
            // RemoteInput fills in the reason, so this intent has to be mutable (it's explicit).
            NoticeAction.Kind.DENY_WHY -> context.getString(R.string.agents_notify_say_why) to
                broadcast(context, AgentActionReceiver.ACTION_DENY_WHY, pcId, threadId, token, mutable = true)
            NoticeAction.Kind.ALLOW -> context.getString(R.string.agents_allow) to
                broadcast(context, AgentActionReceiver.ACTION_ALLOW, pcId, threadId, token, mutable = false)
            NoticeAction.Kind.STOP -> context.getString(R.string.agents_stop) to
                broadcast(context, AgentActionReceiver.ACTION_STOP, pcId, threadId, null, mutable = false)
            // RemoteInput fills in the typed text, so this one intent has to be mutable (it's explicit).
            NoticeAction.Kind.REPLY -> context.getString(R.string.agents_notify_reply) to
                broadcast(context, AgentActionReceiver.ACTION_REPLY, pcId, threadId, null, mutable = true)
            // Threads have no Stop following; a session's own action is built by sessionAction.
            NoticeAction.Kind.UNFOLLOW -> context.getString(R.string.agents_review) to openIntent(context, pcId, threadId)
            // The option's index travels, never its label: the receiver reads the label from the question as it is then.
            NoticeAction.Kind.OPTION -> answers?.options?.getOrNull(spec.option ?: -1).orEmpty() to
                broadcast(context, AgentActionReceiver.ACTION_OPTION, pcId, threadId, token, mutable = false, ask = answers?.askId, option = spec.option)
            NoticeAction.Kind.ANSWER -> context.getString(R.string.agents_notify_type) to
                broadcast(context, AgentActionReceiver.ACTION_ANSWER, pcId, threadId, token, mutable = true, ask = answers?.askId)
        }
        val builder = Notification.Action.Builder(icon, label, intent)
        val input = when (spec.kind) {
            NoticeAction.Kind.REPLY -> RemoteInput.Builder(REPLY_KEY).setLabel(label).build()
            NoticeAction.Kind.DENY_WHY -> RemoteInput.Builder(DENY_WHY_KEY).setLabel(label).build()
            NoticeAction.Kind.ANSWER -> RemoteInput.Builder(ANSWER_KEY).setLabel(context.getString(R.string.agents_notify_type_answer)).build()
            else -> null
        }
        if (input != null) {
            builder.addRemoteInput(input)
            builder.setAllowGeneratedReplies(false)
        }
        if (spec.authenticationRequired && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            builder.setAuthenticationRequired(true)
        }
        return builder.build()
    }

    /** A followed session's one action: Stop following (it turns Keep in status bar off). */
    private fun sessionAction(context: Context, spec: NoticeAction, pcId: String, sessionId: String): Notification.Action {
        val icon = Icon.createWithResource(context, R.drawable.ic_notification)
        val intent = Intent(context, AgentActionReceiver::class.java)
            .setAction(AgentActionReceiver.ACTION_UNFOLLOW)
            .setData(sessionIntentData("unfollow", pcId, sessionId))
            .putExtra(AgentActionReceiver.EXTRA_PC, pcId)
            .putExtra(AgentActionReceiver.EXTRA_SESSION, sessionId)
        val pending = PendingIntent.getBroadcast(
            context,
            (AgentActionReceiver.ACTION_UNFOLLOW + sessionTag(pcId, sessionId)).hashCode(),
            intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val label = when (spec.kind) {
            NoticeAction.Kind.UNFOLLOW -> context.getString(R.string.agents_session_stop_following)
            else -> context.getString(R.string.agents_review)
        }
        return Notification.Action.Builder(icon, label, pending).build()
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

    /** Opens a followed session's view in the app. */
    private fun openSessionIntent(context: Context, pcId: String, sessionId: String): PendingIntent =
        PendingIntent.getActivity(
            context,
            sessionTag(pcId, sessionId).hashCode(),
            Intent(context, MainActivity::class.java)
                .setData(sessionIntentData("open", pcId, sessionId))
                .putExtra(MainActivity.EXTRA_OPEN_AGENT_PC, pcId)
                .putExtra(MainActivity.EXTRA_OPEN_AGENT_SESSION, sessionId)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )

    /** Tells [AgentActionReceiver] the user swiped a Live Update away. */
    private fun dismissIntent(context: Context, tag: String, pcId: String, thread: String?, session: String?): PendingIntent {
        val intent = Intent(context, AgentActionReceiver::class.java)
            .setAction(AgentActionReceiver.ACTION_DISMISSED)
            .setData(Uri.Builder().scheme("g7link-agent-dismissed").authority(pcId).appendPath(thread ?: session.orEmpty()).build())
            .putExtra(AgentActionReceiver.EXTRA_PC, pcId)
        if (thread != null) intent.putExtra(AgentActionReceiver.EXTRA_THREAD, thread)
        if (session != null) intent.putExtra(AgentActionReceiver.EXTRA_SESSION, session)
        return PendingIntent.getBroadcast(
            context,
            (AgentActionReceiver.ACTION_DISMISSED + tag).hashCode(),
            intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
    }

    private fun broadcast(
        context: Context,
        action: String,
        pcId: String,
        threadId: String,
        token: String?,
        mutable: Boolean,
        ask: String? = null,
        option: Int? = null,
    ): PendingIntent {
        // Each option is its own intent (the data differs), so one never takes another's index.
        val kind = if (option != null) "$action/$option" else action
        val intent = Intent(context, AgentActionReceiver::class.java)
            .setAction(action)
            .setData(intentData(kind, pcId, threadId, token))
            .putExtra(AgentActionReceiver.EXTRA_PC, pcId)
            .putExtra(AgentActionReceiver.EXTRA_THREAD, threadId)
        if (token != null) intent.putExtra(AgentActionReceiver.EXTRA_TOKEN, token)
        if (ask != null) intent.putExtra(AgentActionReceiver.EXTRA_ASK, ask)
        if (option != null) intent.putExtra(AgentActionReceiver.EXTRA_OPTION, option)
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

    private fun sessionIntentData(kind: String, pcId: String, sessionId: String): Uri =
        Uri.Builder()
            .scheme("g7link-agent-session")
            .authority(pcId)
            .appendPath(sessionId)
            .appendPath(kind)
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
