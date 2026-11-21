// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.notifications

import android.app.AlarmManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.util.Log
import androidx.core.content.edit
import app.g7link.android.G7LinkApplication
import app.g7link.android.ui.agents.AgentsModel
import app.g7link.android.ui.agents.AgentsState
import app.g7link.android.ui.agents.LimitState
import app.g7link.android.ui.agents.ThreadStatus
import app.g7link.core.AgentThread

/**
 * One limit-reset alarm (contract §V5): a PC's agent whose usage limit the
 * CLI said resets 30 s before [atMs], with what its notification names: the
 * agent, the PC, and the newest thread it paused, when there's one.
 */
data class LimitAlarm(
    val pcId: String,
    val agent: String,
    val atMs: Long,
    val agentName: String,
    val pcName: String,
    val threadId: String? = null,
    val threadTitle: String? = null,
)

/**
 * The rules of the limit-reset alarms, apart from Android so they can be
 * tested. The source is the CLI's own reset time only: a limit it reports
 * reached, or a thread it paused at its limit. The alarm never sends
 * anything; its notification opens the thread (compliance §4).
 */
object LimitAlarms {
    /** The most alarms kept, across PCs. */
    const val MAX = 4

    /** It goes off this long after the reset, so the CLI has caught up. */
    const val AFTER_MS = 30_000L

    /** A reset sooner than this, or later than [MAX_AHEAD_MS], isn't set. */
    const val MIN_AHEAD_MS = 60_000L
    const val MAX_AHEAD_MS = 8L * 24 * 60 * 60 * 1000

    /** What the PC's catalog says of an agent's limits. */
    sealed interface Word {
        /** A limit is reached; [resetsMs] the latest of their resets the CLI gave. */
        data class Reached(val resetsMs: Long?) : Word

        /** Its limits are reported, none reached. */
        data object Clear : Word

        /** It reports none (an older PC, or not reported yet). */
        data object Unknown : Word
    }

    /** When an agent's limit resets as this phone knows. */
    sealed interface Reset {
        data class At(val ms: Long) : Reset

        /** Not reached: no alarm. */
        data object Clear : Reset

        /** Nothing said: whatever is set stays. */
        data object Unknown : Reset
    }

    sealed interface Fired {
        /** Say the limit has reset. */
        data class Post(val alarm: LimitAlarm) : Fired

        /** Still reached on a connected PC: set again for its new time. */
        data class Rearm(val alarm: LimitAlarm) : Fired

        /** Still reached with no new time: nothing more. */
        data object Drop : Fired
    }

    fun word(state: AgentsState, agent: String): Word {
        val limits = state.abilities(agent).limits
        if (limits.isEmpty()) return Word.Unknown
        val reached = limits.filter { it.state == LimitState.REACHED }
        if (reached.isEmpty()) return Word.Clear
        return Word.Reached(reached.mapNotNull { it.resetsMs }.maxOrNull())
    }

    /** The newest thread of [agent] its limit paused. */
    fun pausedThread(state: AgentsState, agent: String): AgentThread? =
        state.threads.filter { it.agent == agent && AgentsModel.status(it) is ThreadStatus.Paused }.maxByOrNull { it.updatedMs }

    fun resetOf(state: AgentsState, agent: String, nowMs: Long): Reset {
        val paused = pausedThread(state, agent)?.let { AgentsModel.status(it) as? ThreadStatus.Paused }?.resetsMs?.takeIf { it > nowMs }
        return when (val word = word(state, agent)) {
            is Word.Reached -> word.resetsMs?.let { Reset.At(it) } ?: paused?.let { Reset.At(it) } ?: Reset.Unknown
            // A thread the limit paused, with a reset still ahead, is the newer word.
            Word.Clear -> paused?.let { Reset.At(it) } ?: Reset.Clear
            Word.Unknown -> paused?.let { Reset.At(it) } ?: Reset.Unknown
        }
    }

    /**
     * Every alarm after [pcId]'s state changed: one per agent whose limit
     * resets between a minute and 8 days ahead, moved when the time moves,
     * gone when it's no longer reached or [enabled] is off; at most [MAX], the
     * soonest kept.
     */
    fun plan(current: List<LimitAlarm>, pcId: String, pcName: String, state: AgentsState, enabled: Boolean, nowMs: Long): List<LimitAlarm> {
        val others = current.filter { it.pcId != pcId }
        if (!enabled) return others
        val mine = current.filter { it.pcId == pcId }.associateBy { it.agent }
        val agents = LinkedHashSet<String>().apply {
            state.catalog?.agents?.forEach { add(it.id) }
            state.threads.forEach { add(it.agent) }
            addAll(mine.keys)
        }
        val next = agents.mapNotNull { agent ->
            val existing = mine[agent]
            when (val reset = resetOf(state, agent, nowMs)) {
                Reset.Unknown -> existing?.let { described(it, state, pcName) }
                Reset.Clear -> null
                is Reset.At -> {
                    val at = reset.ms + AFTER_MS
                    when {
                        reset.ms - nowMs in MIN_AHEAD_MS..MAX_AHEAD_MS -> described(LimitAlarm(pcId, agent, at, agent, pcName), state, pcName)
                        // Too soon or too far to set now; one already set for that time stays.
                        else -> existing?.takeIf { it.atMs == at }?.let { described(it, state, pcName) }
                    }
                }
            }
        }
        return (others + next).sortedBy { it.atMs }.take(MAX)
    }

    /**
     * What an alarm that went off does: on a PC that's connected and still
     * says the limit is reached, nothing (set again for its new time, or
     * dropped without one); otherwise it says the limit has reset.
     */
    fun fired(alarm: LimitAlarm, connected: Boolean, state: AgentsState?, nowMs: Long): Fired {
        if (!connected || state?.catalog == null) return Fired.Post(alarm)
        val word = word(state, alarm.agent)
        if (word is Word.Reached) {
            val resets = word.resetsMs ?: return Fired.Drop
            // A reset already past is the catalog not caught up: the CLI's time stands.
            if (resets > nowMs) {
                return if (resets - nowMs <= MAX_AHEAD_MS) Fired.Rearm(described(alarm.copy(atMs = resets + AFTER_MS), state, alarm.pcName)) else Fired.Drop
            }
        }
        return Fired.Post(described(alarm, state, alarm.pcName))
    }

    /** [alarm] with the agent's name and its newest paused thread as [state] has them. */
    private fun described(alarm: LimitAlarm, state: AgentsState, pcName: String): LimitAlarm {
        val name = if (state.catalog != null) state.agentName(alarm.agent) else alarm.agentName
        val paused = pausedThread(state, alarm.agent)
        return alarm.copy(
            agentName = name,
            pcName = pcName.ifBlank { alarm.pcName },
            threadId = paused?.id ?: alarm.threadId.takeIf { state.threads.isEmpty() },
            threadTitle = paused?.title?.ifBlank { null } ?: alarm.threadTitle.takeIf { state.threads.isEmpty() },
        )
    }

    private const val SEP = '\u001f'

    /** One alarm as one line in the app's preferences. */
    fun encode(alarm: LimitAlarm): String = listOf(
        alarm.pcId,
        alarm.agent,
        alarm.atMs.toString(),
        plain(alarm.agentName),
        plain(alarm.pcName),
        alarm.threadId.orEmpty(),
        plain(alarm.threadTitle.orEmpty()),
    ).joinToString(SEP.toString())

    fun decode(line: String): LimitAlarm? {
        val parts = line.split(SEP)
        if (parts.size != 7) return null
        val pc = parts[0].takeIf { AgentsModel.isId(it, max = AgentsModel.MAX_PC_ID) } ?: return null
        val agent = parts[1].takeIf { AgentsModel.isId(it) } ?: return null
        val at = parts[2].toLongOrNull()?.takeIf { it > 0L } ?: return null
        val thread = parts[5].ifEmpty { null }
        if (thread != null && !AgentsModel.isId(thread)) return null
        return LimitAlarm(pc, agent, at, parts[3].ifEmpty { agent }, parts[4], thread, parts[6].ifEmpty { null })
    }

    private fun plain(text: String): String = text.map { if (it.isISOControl()) ' ' else it }.joinToString("").take(MAX_TEXT_CHARS)

    private const val MAX_TEXT_CHARS = 256
}

/**
 * The limit-reset alarms on this phone (contract §V5): kept in the app's
 * preferences, set with `AlarmManager.setAndAllowWhileIdle` (inexact, no
 * exact-alarm permission), set again when the app starts, and handled by
 * [AgentLimitAlarmReceiver].
 */
object AgentLimitAlarm {
    private const val TAG = "AgentLimitAlarm"
    private const val PREFS = "agent_limit_alarms"
    private const val KEY = "alarms"
    const val ACTION_FIRE = "app.g7link.android.agents.LIMIT_RESET"
    const val EXTRA_PC = "agent_pc"
    const val EXTRA_AGENT = "agent_agent"

    @Synchronized
    fun alarms(context: Context): List<LimitAlarm> =
        prefs(context).getStringSet(KEY, null).orEmpty().mapNotNull(LimitAlarms::decode).sortedBy { it.atMs }

    /** The alarm set for [agent] on [pcId], if any ("you'll be told"). */
    fun armedFor(context: Context, pcId: String, agent: String): LimitAlarm? =
        runCatching { alarms(context).firstOrNull { it.pcId == pcId && it.agent == agent } }.getOrNull()

    /** A PC's agents changed: its alarms follow. */
    @Synchronized
    fun update(context: Context, pcId: String, pcName: String, state: AgentsState) {
        val enabled = AgentNoticeSettings.limitAlarm(context, pcId)
        val before = alarms(context)
        val after = LimitAlarms.plan(before, pcId, pcName, state, enabled, System.currentTimeMillis())
        if (after != before) store(context, before, after)
    }

    /** The PC was unpaired, or its setting turned off: its alarms go, and what they posted. */
    @Synchronized
    fun forgetPc(context: Context, pcId: String) {
        val before = alarms(context)
        val after = before.filter { it.pcId != pcId }
        if (after != before) store(context, before, after)
        AgentNotifications.cancelLimitResets(context, pcId)
    }

    /** Sets every kept alarm again (an alarm doesn't outlive a restart of the phone). */
    @Synchronized
    fun rearm(context: Context) {
        val now = System.currentTimeMillis()
        val kept = alarms(context)
        // One whose time passed while nothing ran goes off now.
        kept.forEach { schedule(context, it.copy(atMs = maxOf(it.atMs, now + LimitAlarms.AFTER_MS))) }
    }

    /** An alarm went off. */
    @Synchronized
    fun fire(context: Context, pcId: String, agent: String) {
        val all = alarms(context)
        val alarm = all.firstOrNull { it.pcId == pcId && it.agent == agent } ?: return
        val rest = all - alarm
        if (!AgentNoticeSettings.limitAlarm(context, pcId)) {
            store(context, all, rest)
            return
        }
        val core = (context.applicationContext as? G7LinkApplication)?.core
        val connected = core?.state?.value?.device(pcId)?.online == true
        val state = core?.agents?.value?.get(pcId)
        when (val fired = LimitAlarms.fired(alarm, connected, state, System.currentTimeMillis())) {
            is LimitAlarms.Fired.Post -> {
                store(context, all, rest)
                AgentNotifications.limitReset(context, fired.alarm)
            }
            is LimitAlarms.Fired.Rearm -> store(context, all, rest + fired.alarm)
            LimitAlarms.Fired.Drop -> store(context, all, rest)
        }
    }

    private fun store(context: Context, before: List<LimitAlarm>, after: List<LimitAlarm>) {
        val keys = { a: LimitAlarm -> a.pcId to a.agent }
        val kept = after.associateBy(keys)
        before.filter { kept[keys(it)]?.atMs != it.atMs }.forEach { cancel(context, it) }
        prefs(context).edit { putStringSet(KEY, after.map(LimitAlarms::encode).toSet()) }
        val was = before.associateBy(keys)
        after.filter { was[keys(it)]?.atMs != it.atMs }.forEach { schedule(context, it) }
    }

    private fun schedule(context: Context, alarm: LimitAlarm) {
        val manager = context.getSystemService(AlarmManager::class.java) ?: return
        runCatching { manager.setAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, alarm.atMs, intent(context, alarm.pcId, alarm.agent)) }
            .onFailure { Log.w(TAG, "can't set the limit alarm", it) }
    }

    private fun cancel(context: Context, alarm: LimitAlarm) {
        val manager = context.getSystemService(AlarmManager::class.java) ?: return
        runCatching { manager.cancel(intent(context, alarm.pcId, alarm.agent)) }
    }

    private fun intent(context: Context, pcId: String, agent: String): PendingIntent = PendingIntent.getBroadcast(
        context,
        "agent-limit:$pcId:$agent".hashCode(),
        Intent(context, AgentLimitAlarmReceiver::class.java)
            .setAction(ACTION_FIRE)
            .setData(Uri.Builder().scheme("g7link-agent-limit").authority(pcId).appendPath(agent).build())
            .putExtra(EXTRA_PC, pcId)
            .putExtra(EXTRA_AGENT, agent),
        PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
    )

    private fun prefs(context: Context) = context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
}
