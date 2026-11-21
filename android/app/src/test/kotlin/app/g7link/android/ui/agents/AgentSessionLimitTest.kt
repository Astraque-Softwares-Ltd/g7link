// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import app.g7link.core.AgentSession
import app.g7link.core.AgentSessionLimit
import app.g7link.core.AgentSessionSource
import app.g7link.core.AgentSessionState
import java.util.TimeZone
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** A followed session that stopped at its agent's usage limit (owner's report, 2026-11-21). */
class AgentSessionLimitTest {
    private val now = 1_795_000_000_000L
    private val hour = 60L * 60 * 1000

    private fun session(state: AgentSessionState, guess: Boolean = false, limit: AgentSessionLimit? = null) = AgentSession(
        id = "sc0123456789abcdef0123456789abcdef",
        agent = "claude",
        folder = "safari-web",
        title = "Tafsiri ya ukurasa wa malipo",
        state = state,
        guess = guess,
        source = AgentSessionSource.TERMINAL,
        updatedMs = now,
        step = null,
        summary = null,
        waitingFor = null,
        canContinue = false,
        blocked = null,
        sinceMs = now - hour,
        limit = limit,
    )

    @Test
    fun `a session at its usage limit reads as limited, never as working`() {
        val resets = now + 2 * hour
        val limited = session(AgentSessionState.IDLE, limit = AgentSessionLimit(resets))
        assertEquals(SessionLook.LIMITED, AgentsModel.sessionLook(limited))
        assertEquals(resets, AgentsModel.sessionLimitResets(limited, now))
        // Whatever the state says: a live "busy" the PC hasn't caught up with doesn't make it working.
        assertEquals(SessionLook.LIMITED, AgentsModel.sessionLook(session(AgentSessionState.WORKING, limit = AgentSessionLimit(resets))))
        // No time said, or a time already past: "Limit reached" alone.
        val untimed = session(AgentSessionState.IDLE, limit = AgentSessionLimit(null))
        assertEquals(SessionLook.LIMITED, AgentsModel.sessionLook(untimed))
        assertNull(AgentsModel.sessionLimitResets(untimed, now))
        assertNull(AgentsModel.sessionLimitResets(limited, resets))
    }

    @Test
    fun `without a limit a session reads as before`() {
        assertEquals(SessionLook.WORKING, AgentsModel.sessionLook(session(AgentSessionState.WORKING)))
        assertEquals(SessionLook.NEEDS_YOU, AgentsModel.sessionLook(session(AgentSessionState.NEEDS_YOU)))
        assertEquals(SessionLook.MAY_NEED_YOU, AgentsModel.sessionLook(session(AgentSessionState.NEEDS_YOU, guess = true)))
        assertEquals(SessionLook.MAY_NEED_YOU, AgentsModel.sessionLook(session(AgentSessionState.MAY_NEED_YOU)))
        assertEquals(SessionLook.IDLE, AgentsModel.sessionLook(session(AgentSessionState.IDLE)))
        assertEquals(SessionLook.ENDED, AgentsModel.sessionLook(session(AgentSessionState.ENDED)))
        assertNull(AgentsModel.sessionLimitResets(session(AgentSessionState.IDLE), now))
    }

    @Test
    fun `a reset today says its time, another day its weekday too`() {
        val nairobi = TimeZone.getTimeZone("Africa/Nairobi")
        // 2026-11-21 14:32 in Nairobi (UTC+3).
        val at = 1_795_260_720_000L
        assertTrue(AgentsModel.sameDay(at + 3 * hour, at, nairobi))
        assertFalse(AgentsModel.sameDay(at + 10 * hour, at, nairobi))
        assertFalse(AgentsModel.sameDay(at + 7 * 24 * hour, at, nairobi))
        // The same instant is another day where it's already past midnight.
        assertFalse(AgentsModel.sameDay(at + 9 * hour + 30 * 60 * 1000, at, nairobi))
        assertTrue(AgentsModel.sameDay(at + 9 * hour + 30 * 60 * 1000, at, TimeZone.getTimeZone("UTC")))
    }
}
