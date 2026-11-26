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
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The Agents list by folder: today's sessions beside their threads, what needs you first, folders that remember being opened. */
class AgentProjectsTest {
    private val day = 24 * 60 * 60_000L
    private val now = 1_795_000_000_000L

    private fun thread(
        id: String,
        folder: String,
        state: AgentThreadState = AgentThreadState.DONE,
        updatedMs: Long = now,
        asks: UInt = 0u,
        pinned: Boolean = false,
        archived: Boolean = false,
        snoozedUntilMs: Long? = null,
    ) = AgentThread(
        id = id,
        agent = "claude",
        folder = folder,
        title = "Thread $id",
        state = state,
        mode = AgentMode.ASK,
        fromPhone = true,
        updatedMs = updatedMs,
        asks = asks,
        summary = null,
        error = null,
        pinned = pinned,
        archived = archived,
        snoozedUntilMs = snoozedUntilMs,
    )

    private fun session(id: String, folder: String, state: AgentSessionState, updatedMs: Long = now, guess: Boolean = false) = AgentSession(
        id = id,
        agent = "codex",
        folder = folder,
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

    private fun state(threads: List<AgentThread>, sessions: List<AgentSession> = emptyList()): AgentsState {
        val withThreads = AgentsModel.withThreads(AgentsState(), threads)
        return if (sessions.isEmpty()) withThreads else AgentsModel.withSessions(withThreads, sessions)
    }

    @Test
    fun `a session waiting today puts its folder above weeks-old threads`() {
        val old = (1..10).map { thread("old$it", "mto-api", updatedMs = now - 21 * day - it) }
        val groups = AgentProjects.groups(
            state(old, listOf(session("s1", "kilima-web", AgentSessionState.NEEDS_YOU, updatedMs = now - 60_000L))),
        )
        assertEquals(listOf("kilima-web", "mto-api"), groups.map { it.folder })
        assertEquals(1, groups[0].needsYou)
        assertEquals(1, groups[0].sessions)
        assertEquals(10, groups[1].threads)
    }

    @Test
    fun `folders go needs you, then running, then the most recently changed`() {
        val groups = AgentProjects.groups(
            state(
                listOf(
                    thread("quiet", "c-latest", updatedMs = now),
                    thread("runs", "b-running", state = AgentThreadState.WORKING, updatedMs = now - 3 * day),
                    thread("asks", "a-waits", state = AgentThreadState.WORKING, asks = 1u, updatedMs = now - 5 * day),
                    thread("older", "d-older", updatedMs = now - day),
                ),
            ),
        )
        assertEquals(listOf("a-waits", "b-running", "c-latest", "d-older"), groups.map { it.folder })
    }

    @Test
    fun `inside a folder rows go needs you, may need you, running, pinned, then newest, threads and sessions together`() {
        val group = AgentProjects.groups(
            state(
                listOf(
                    thread("done-new", "nyumba", updatedMs = now),
                    thread("pinned", "nyumba", pinned = true, updatedMs = now - 5 * day),
                    thread("waits", "nyumba", state = AgentThreadState.WAITING, updatedMs = now - 2 * day),
                    thread("works", "nyumba", state = AgentThreadState.WORKING, updatedMs = now - day),
                ),
                listOf(
                    session("guess", "nyumba", AgentSessionState.NEEDS_YOU, updatedMs = now - 3 * day, guess = true),
                    session("ended", "nyumba", AgentSessionState.ENDED, updatedMs = now - 60_000L),
                    session("busy", "nyumba", AgentSessionState.WORKING, updatedMs = now - 4 * day),
                ),
            ),
        ).single()
        assertEquals(
            listOf("thread:waits", "session:guess", "thread:works", "session:busy", "thread:pinned", "thread:done-new", "session:ended"),
            group.items.map { it.key },
        )
        // A guess never counts, but it keeps the folder open.
        assertEquals(1, group.needsYou)
        assertEquals(1, group.mayNeedYou)
        assertEquals(2, group.running)
        assertEquals(now, group.lastMs)
    }

    @Test
    fun `archived and snoozed threads stay on their shelves, and sessions show only while the PC lists them`() {
        val threads = listOf(
            thread("main", "pwani"),
            thread("archived", "pwani", archived = true),
            thread("snoozed", "pwani", snoozedUntilMs = now + day),
        )
        val sessions = listOf(session("s1", "pwani", AgentSessionState.IDLE, updatedMs = now - 1))
        val listed = state(threads, sessions)
        assertEquals(listOf("thread:main", "session:s1"), AgentProjects.groups(listed).single().items.map { it.key })
        assertEquals(listOf("thread:main"), AgentProjects.groups(AgentsModel.sessionsOff(listed)).single().items.map { it.key })
        assertEquals(listOf("thread:main"), AgentProjects.groups(AgentsModel.sessionsUnsupported(listed)).single().items.map { it.key })
    }

    @Test
    fun `the pager holds threads that need you and sessions that need you as reported, newest first`() {
        val cards = AgentProjects.cards(
            state(
                listOf(
                    thread("asks", "a", state = AgentThreadState.WORKING, asks = 1u, updatedMs = now - 2 * day),
                    thread("waits", "b", state = AgentThreadState.WAITING, updatedMs = now - day),
                    thread("done", "c"),
                    thread("archived", "c", state = AgentThreadState.WAITING, archived = true),
                ),
                listOf(
                    session("exact", "d", AgentSessionState.NEEDS_YOU, updatedMs = now),
                    session("guess", "d", AgentSessionState.NEEDS_YOU, updatedMs = now, guess = true),
                    session("works", "d", AgentSessionState.WORKING),
                ),
            ),
        )
        assertEquals(listOf("session:exact", "thread:waits", "thread:asks"), cards.map { it.key })
    }

    @Test
    fun `a folder starts open when it needs you, may, runs, or changed last, and then keeps the user's choice`() {
        val groups = AgentProjects.groups(
            state(
                listOf(
                    thread("waits", "waits", state = AgentThreadState.WAITING, updatedMs = now - 3 * day),
                    thread("newest", "newest", updatedMs = now),
                    thread("quiet", "quiet", updatedMs = now - 9 * day),
                ),
                listOf(session("guess", "guess", AgentSessionState.MAY_NEED_YOU, updatedMs = now - 8 * day)),
            ),
        )
        val newest = AgentProjects.newest(groups)
        assertEquals("newest", newest)
        val byFolder = groups.associateBy { it.folder }
        assertTrue(AgentProjects.open(byFolder.getValue("waits"), newest, emptyMap()))
        assertTrue(AgentProjects.open(byFolder.getValue("guess"), newest, emptyMap()))
        assertTrue(AgentProjects.open(byFolder.getValue("newest"), newest, emptyMap()))
        assertFalse(AgentProjects.open(byFolder.getValue("quiet"), newest, emptyMap()))
        val chosen = AgentProjects.chose(AgentProjects.chose(emptyMap(), "waits", open = false), "quiet", open = true)
        assertFalse(AgentProjects.open(byFolder.getValue("waits"), newest, chosen))
        assertTrue(AgentProjects.open(byFolder.getValue("quiet"), newest, chosen))
    }

    @Test
    fun `a folder shows what needs you or runs and three more, the rest behind more`() {
        val group = AgentProjects.groups(
            state(
                listOf(thread("works", "f", state = AgentThreadState.WORKING)) +
                    (1..6).map { thread("done$it", "f", updatedMs = now - it * day) },
            ),
        ).single()
        assertEquals(4, group.firstShown)
        assertEquals(listOf("thread:works", "thread:done1", "thread:done2", "thread:done3"), AgentProjects.shown(group, all = false).map { it.key })
        assertEquals(7, AgentProjects.shown(group, all = true).size)
        val small = AgentProjects.groups(state(listOf(thread("one", "g")))).single()
        assertEquals(1, small.firstShown)
    }

    @Test
    fun `choices are kept newest last, capped, and read back from what was stored`() {
        var chosen = emptyMap<String, Boolean>()
        for (i in 0 until AgentProjects.MAX_REMEMBERED + 5) chosen = AgentProjects.chose(chosen, "folder$i", open = i % 2 == 0)
        assertEquals(AgentProjects.MAX_REMEMBERED, chosen.size)
        assertNull(chosen["folder0"])
        assertEquals(true, chosen["folder${AgentProjects.MAX_REMEMBERED + 4}"])
        // Choosing again moves a folder to the end.
        chosen = AgentProjects.chose(chosen, "folder10", open = true)
        assertEquals("folder10", chosen.keys.last())
        assertEquals(chosen, AgentProjects.decode(AgentProjects.encode(chosen)))
    }

    @Test
    fun `what can't be stored is skipped, and anything malformed is read as nothing`() {
        val stored = AgentProjects.encode(linkedMapOf("ok" to true, "two\nlines" to false, "x".repeat(65) to true, "" to false))
        assertEquals(linkedMapOf("ok" to true, "" to false), AgentProjects.decode(stored))
        assertEquals(emptyMap<String, Boolean>(), AgentProjects.decode(null))
        assertEquals(mapOf("kept" to false), AgentProjects.decode("junk\n2\tnope\n1nope\n0\tkept"))
    }
}
