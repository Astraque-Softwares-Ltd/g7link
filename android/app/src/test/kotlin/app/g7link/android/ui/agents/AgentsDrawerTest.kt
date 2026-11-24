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

/** The drawer groups threads and the sessions started on the PC together, newest first, and lists sessions only where the PC lists them. */
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

    private fun session(id: String, state: AgentSessionState = AgentSessionState.WORKING, updatedMs: Long = 1L, guess: Boolean = false) = AgentSession(
        id = id,
        agent = "codex",
        folder = "g7linux",
        title = "Session $id",
        state = state,
        guess = guess,
        source = AgentSessionSource.TERMINAL,
        updatedMs = updatedMs,
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
        assertFalse(groups.empty)
    }

    @Test
    fun `sessions show only while the PC lists them to this phone`() {
        val listed = AgentsModel.withSessions(AgentsState(), listOf(session("sc01")))
        assertEquals(listOf("sc01"), AgentsModel.drawerGroups(listed).working.map { it.id })
        assertTrue(AgentsModel.drawerGroups(AgentsModel.sessionsOff(listed)).empty)
        assertTrue(AgentsModel.drawerGroups(AgentsModel.sessionsUnsupported(listed)).empty)
        assertTrue(AgentsModel.drawerGroups(AgentsState()).empty)
    }

    @Test
    fun `a session sits in its group beside the threads, by time, never below them all`() {
        val threads = AgentsModel.withThreads(
            AgentsState(),
            listOf(
                thread("old", AgentThreadState.DONE, updatedMs = 10L),
                thread("older", AgentThreadState.DONE, updatedMs = 5L),
                thread("asks", AgentThreadState.WORKING, updatedMs = 30L, asks = 1u),
                thread("works", AgentThreadState.WORKING, updatedMs = 20L),
            ),
        )
        val state = AgentsModel.withSessions(
            threads,
            listOf(
                session("waits", AgentSessionState.NEEDS_YOU, updatedMs = 40L),
                session("guess", AgentSessionState.NEEDS_YOU, updatedMs = 50L, guess = true),
                session("busy", AgentSessionState.WORKING, updatedMs = 25L),
                session("idle", AgentSessionState.IDLE, updatedMs = 15L),
                session("ended", AgentSessionState.ENDED, updatedMs = 1L),
            ),
        )
        val groups = AgentsModel.drawerGroups(state)
        // Reported waits first (newest first), the guessed one after them.
        assertEquals(listOf("waits", "asks", "guess"), groups.needsYou.map { it.id })
        assertEquals(listOf("busy", "works"), groups.working.map { it.id })
        assertEquals(listOf("idle", "old", "older", "ended"), groups.recent.map { it.id })
    }
}
