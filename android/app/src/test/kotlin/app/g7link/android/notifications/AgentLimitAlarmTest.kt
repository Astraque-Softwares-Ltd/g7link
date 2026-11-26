// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.notifications

import app.g7link.android.ui.agents.AgentsModel
import app.g7link.android.ui.agents.AgentsState
import app.g7link.core.AgentCatalog
import app.g7link.core.AgentInfo
import app.g7link.core.AgentLimit
import app.g7link.core.AgentLimitState
import app.g7link.core.AgentMode
import app.g7link.core.AgentThread
import app.g7link.core.AgentThreadState
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The limit-reset alarm's rules (contract §V5). */
class AgentLimitAlarmTest {
    private val pc = "pc1"
    private val now = 1_795_000_000_000L
    private val hour = 60L * 60 * 1000

    private fun info(id: String, limits: List<AgentLimit>) =
        AgentInfo(id = id, name = id.replaceFirstChar { it.uppercaseChar() }, installed = true, signedIn = true, loginHint = "", limits = limits)

    private fun limit(state: AgentLimitState, resetsMs: Long? = null, window: String = "5h") =
        AgentLimit(window = window, usedPct = null, resetsMs = resetsMs, state = state)

    private fun catalog(vararg agents: AgentInfo) =
        AgentCatalog(agents = agents.toList(), folders = emptyList(), canControl = true)

    private fun paused(id: String = "t1", agent: String = "claude", resetsMs: Long?, updatedMs: Long = 1_000L) = AgentThread(
        id = id,
        agent = agent,
        folder = "safari-web",
        title = "Tafsiri ya ukurasa wa malipo",
        state = AgentThreadState.FAILED,
        mode = AgentMode.ASK,
        fromPhone = true,
        updatedMs = updatedMs,
        asks = 0u,
        summary = null,
        error = "Usage limit reached" + (resetsMs?.let { "|resets_ms=$it" } ?: ""),
        errorCode = "usage_limit",
    )

    private fun state(catalog: AgentCatalog?, vararg threads: AgentThread) =
        AgentsModel.withThreads(AgentsState(catalog = catalog), threads.toList())

    @Test
    fun `a reached limit with a reset ahead sets one alarm 30 s after it`() {
        val resets = now + 2 * hour
        val alarms = LimitAlarms.plan(
            emptyList(),
            pc,
            "Kilele",
            state(catalog(info("claude", listOf(limit(AgentLimitState.REACHED, resets)))), paused(resetsMs = resets)),
            enabled = true,
            nowMs = now,
        )
        val alarm = alarms.single()
        assertEquals(resets + LimitAlarms.AFTER_MS, alarm.atMs)
        assertEquals("Claude", alarm.agentName)
        assertEquals("Kilele", alarm.pcName)
        assertEquals("t1", alarm.threadId)
        assertEquals("Tafsiri ya ukurasa wa malipo", alarm.threadTitle)

        // A thread paused at its limit is enough where the PC reports no limits.
        val fromThread = LimitAlarms.plan(emptyList(), pc, "Kilele", state(catalog(info("claude", emptyList())), paused(resetsMs = resets)), true, now)
        assertEquals(resets + LimitAlarms.AFTER_MS, fromThread.single().atMs)
        // The setting off: nothing.
        assertTrue(LimitAlarms.plan(emptyList(), pc, "Kilele", state(catalog(info("claude", listOf(limit(AgentLimitState.REACHED, resets))))), false, now).isEmpty())
    }

    @Test
    fun `the alarm moves with the time, and goes when the limit isn't reached`() {
        val first = now + hour
        val start = LimitAlarms.plan(emptyList(), pc, "Kilele", state(catalog(info("claude", listOf(limit(AgentLimitState.REACHED, first))))), true, now)
        val later = now + 3 * hour
        val moved = LimitAlarms.plan(start, pc, "Kilele", state(catalog(info("claude", listOf(limit(AgentLimitState.REACHED, later))))), true, now)
        assertEquals(later + LimitAlarms.AFTER_MS, moved.single().atMs)
        // Not reached any more: cancelled.
        assertTrue(LimitAlarms.plan(moved, pc, "Kilele", state(catalog(info("claude", listOf(limit(AgentLimitState.OK))))), true, now).isEmpty())
        // The PC says nothing of limits: what's set stays.
        assertEquals(moved, LimitAlarms.plan(moved, pc, "Kilele", state(catalog(info("claude", emptyList()))), true, now))
        // Turned off: this PC's go, another PC's stay.
        val other = LimitAlarm("pc2", "codex", now + hour, "Codex", "Mlima")
        assertEquals(listOf(other), LimitAlarms.plan(moved + other, pc, "Kilele", AgentsState(), false, now))
    }

    @Test
    fun `only a reset between a minute and 8 days ahead is set`() {
        val plan = { resets: Long -> LimitAlarms.plan(emptyList(), pc, "Kilele", state(catalog(info("claude", listOf(limit(AgentLimitState.REACHED, resets))))), true, now) }
        assertTrue(plan(now + 30_000L).isEmpty())
        assertEquals(1, plan(now + LimitAlarms.MIN_AHEAD_MS).size)
        assertEquals(1, plan(now + LimitAlarms.MAX_AHEAD_MS).size)
        assertTrue(plan(now + LimitAlarms.MAX_AHEAD_MS + 1).isEmpty())
        assertTrue(plan(now - hour).isEmpty())
        // A reached limit without a time sets nothing.
        assertTrue(LimitAlarms.plan(emptyList(), pc, "Kilele", state(catalog(info("claude", listOf(limit(AgentLimitState.REACHED))))), true, now).isEmpty())
    }

    @Test
    fun `at most 4 alarms are kept, the soonest`() {
        val existing = (1..4).map { LimitAlarm("pc${it + 1}", "claude", now + it * hour, "Claude", "PC $it") }
        val resets = now + 30 * 60 * 1000L
        val next = LimitAlarms.plan(existing, pc, "Kilele", state(catalog(info("claude", listOf(limit(AgentLimitState.REACHED, resets))))), true, now)
        assertEquals(LimitAlarms.MAX, next.size)
        assertEquals(pc, next.first().pcId)
        assertTrue(next.none { it.pcId == "pc5" })
    }

    @Test
    fun `when it goes off on a PC that still says reached, nothing is posted`() {
        val alarm = LimitAlarm(pc, "claude", now, "Claude", "Kilele", "t1", "Tafsiri ya ukurasa wa malipo")
        val later = now + 2 * hour
        val stillReached = state(catalog(info("claude", listOf(limit(AgentLimitState.REACHED, later)))), paused(resetsMs = now - 30_000L))
        val rearmed = LimitAlarms.fired(alarm, connected = true, state = stillReached, nowMs = now) as LimitAlarms.Fired.Rearm
        assertEquals(later + LimitAlarms.AFTER_MS, rearmed.alarm.atMs)
        // Still reached, no new time: dropped.
        val noTime = state(catalog(info("claude", listOf(limit(AgentLimitState.REACHED)))))
        assertEquals(LimitAlarms.Fired.Drop, LimitAlarms.fired(alarm, connected = true, state = noTime, nowMs = now))

        // Reset on the PC: posted, naming the newest thread the limit paused.
        val clear = state(
            catalog(info("claude", listOf(limit(AgentLimitState.OK)))),
            paused("t1", resetsMs = now - 30_000L, updatedMs = 1_000L),
            paused("t2", resetsMs = now - 30_000L, updatedMs = 2_000L).copy(title = "Newer"),
        )
        val posted = LimitAlarms.fired(alarm, connected = true, state = clear, nowMs = now) as LimitAlarms.Fired.Post
        assertEquals("t2", posted.alarm.threadId)
        assertEquals("Newer", posted.alarm.threadTitle)
        // Not connected: posted from what it kept.
        assertEquals(LimitAlarms.Fired.Post(alarm), LimitAlarms.fired(alarm, connected = false, state = null, nowMs = now))
        // A reached limit whose time already passed is the catalog not caught up: posted.
        val stale = state(catalog(info("claude", listOf(limit(AgentLimitState.REACHED, now - hour)))))
        assertTrue(LimitAlarms.fired(alarm, connected = true, state = stale, nowMs = now) is LimitAlarms.Fired.Post)
    }

    @Test
    fun `an alarm survives a restart as one line, and a bad line is dropped`() {
        val alarm = LimitAlarm(pc, "claude", now, "Claude", "Kilele", "t1", "Line one\nline two")
        val back = LimitAlarms.decode(LimitAlarms.encode(alarm))!!
        assertEquals(alarm.copy(threadTitle = "Line one line two"), back)
        val bare = LimitAlarm(pc, "codex", now, "Codex", "Kilele")
        assertEquals(bare, LimitAlarms.decode(LimitAlarms.encode(bare)))
        assertNull(LimitAlarms.decode("../etc\u001fclaude\u001f1\u001fClaude\u001fKilele\u001f\u001f"))
        assertNull(LimitAlarms.decode("pc1\u001fclaude\u001fsoon\u001fClaude\u001fKilele\u001f\u001f"))
        assertNull(LimitAlarms.decode("garbage"))
    }
}
