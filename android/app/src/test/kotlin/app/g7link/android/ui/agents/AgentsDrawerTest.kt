// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import app.g7link.core.AgentMode
import app.g7link.core.AgentSession
import app.g7link.core.AgentSessionSource
import app.g7link.core.AgentSessionState
import app.g7link.core.AgentThread
import app.g7link.core.AgentThreadState
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** The drawer groups threads as the list does, and lists sessions only where the PC lists them. */
class AgentsDrawerTest {
    private fun thread(id: String, state: AgentThreadState, updatedMs: Long, asks: UInt = 0u) = AgentThread(
        id = id,
        agent = "claude",
        folder = "g7linux-phone",
        title = "Thread $id",
        state = state,
        mode = AgentMode.SUPERVISED,
        fromPhone = true,
        updatedMs = updatedMs,
        asks = asks,
        summary = null,
        error = null,
    )

    private fun session(id: String) = AgentSession(
        id = id,
        agent = "codex",
        folder = "g7linux",
        title = "Session $id",
        state = AgentSessionState.WORKING,
        guess = false,
        source = AgentSessionSource.TERMINAL,
        updatedMs = 1L,
        step = null,
        summary = null,
        waitingFor = null,
        canContinue = false,
        blocked = null,
    )

    @Test
    fun `threads fall into needs you, working and recent, in the list's order`() {
        val state = AgentsModel.withThreads(
            AgentsState(),
            listOf(
                thread("done", AgentThreadState.DONE, updatedMs = 9L),
                thread("asks", AgentThreadState.WORKING, updatedMs = 2L, asks = 1u),
                thread("waits", AgentThreadState.WAITING, updatedMs = 5L),
                thread("works", AgentThreadState.WORKING, updatedMs = 7L),
                thread("failed", AgentThreadState.FAILED, updatedMs = 3L),
                thread("stopped", AgentThreadState.INTERRUPTED, updatedMs = 8L),
            ),
        )
        val groups = AgentsModel.drawerGroups(state)
        assertEquals(listOf("waits", "asks"), groups.needsYou.map { it.id })
        assertEquals(listOf("works"), groups.working.map { it.id })
        assertEquals(listOf("done", "stopped", "failed"), groups.recent.map { it.id })
        assertTrue(groups.sessions.isEmpty())
        assertFalse(groups.empty)
    }

    @Test
    fun `sessions show only while the PC lists them to this phone`() {
        val listed = AgentsModel.withSessions(AgentsState(), listOf(session("sc01")))
        assertEquals(listOf("sc01"), AgentsModel.drawerGroups(listed).sessions.map { it.id })
        assertTrue(AgentsModel.drawerGroups(AgentsModel.sessionsOff(listed)).sessions.isEmpty())
        assertTrue(AgentsModel.drawerGroups(AgentsModel.sessionsUnsupported(listed)).sessions.isEmpty())
        assertTrue(AgentsModel.drawerGroups(AgentsState()).empty)
    }
}
