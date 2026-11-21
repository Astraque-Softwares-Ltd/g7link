// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.agents

import app.g7link.android.ui.agents.AgentsModel
import app.g7link.android.ui.agents.AgentsState
import app.g7link.core.AgentAsk
import app.g7link.core.AgentAskKind
import app.g7link.core.AgentCatalog
import app.g7link.core.AgentInfo
import app.g7link.core.AgentMode
import app.g7link.core.AgentThread
import app.g7link.core.AgentThreadState
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The Agents tile and widget (contract §V8.3): what they show, from a snapshot that holds only that. */
class AgentsSurfaceTest {
    private val pc = "pc1"
    private val command = "rm -rf ./build && cargo test --release"

    private val catalog = AgentCatalog(
        agents = listOf(AgentInfo(id = "claude", name = "Claude", installed = true, signedIn = true, loginHint = "claude login")),
        folders = emptyList(),
        modes = listOf(AgentMode.SUPERVISED),
    )

    private fun thread(id: String, state: AgentThreadState, updatedMs: Long = 1_000L, unread: Boolean = false, archived: Boolean = false, asks: UInt = 0u) =
        AgentThread(
            id = id,
            agent = "claude",
            folder = "duka-pos",
            title = "Thread $id",
            state = state,
            mode = AgentMode.SUPERVISED,
            fromPhone = false,
            updatedMs = updatedMs,
            asks = asks,
            summary = "Summary of $id",
            error = null,
            unread = unread,
            archived = archived,
        )

    private fun ask(token: String, threadId: String, expiresMs: Long) = AgentAsk(
        token = token,
        thread = threadId,
        threadTitle = "Thread $threadId",
        agent = "claude",
        kind = AgentAskKind.COMMAND,
        title = "Run command",
        command = command,
        diff = null,
        path = null,
        reason = null,
        truncated = false,
        expiresMs = expiresMs,
    )

    private fun state(vararg threads: AgentThread, asks: List<AgentAsk> = emptyList()): AgentsState {
        val base = AgentsModel.withThreads(AgentsState(catalog = catalog), threads.toList())
        return base.copy(asks = asks.associateBy { it.token })
    }

    @Test
    fun `the snapshot holds the title, agent, folder and state, and never a request's command`() {
        val s = state(thread("t1", AgentThreadState.WAITING, asks = 1u), asks = listOf(ask("a1", "t1", 9_000L)))
        val pcPart = AgentsSurfaces.of(pc, "Kilele", s, online = true, nowMs = 5L)!!
        assertEquals(listOf(SurfaceRow("t1", "Thread t1", "Claude", "duka-pos", SurfaceState.NEEDS_YOU)), pcPart.rows)
        val written = AgentsSurfaces.encode(AgentsSurface(listOf(pcPart)))
        assertFalse(written.contains(command))
        assertFalse(written.contains("cargo"))
        assertFalse(written.contains("Summary of"))
        assertFalse(written.contains("Run command"))
        assertEquals(AgentsSurface(listOf(pcPart)), AgentsSurfaces.decode(written))
    }

    @Test
    fun `a PC that offers no agents isn't on the surfaces`() {
        assertNull(AgentsSurfaces.of(pc, "Kilele", AgentsState(), online = true, nowMs = 1L))
    }

    @Test
    fun `rows put what needs you first, oldest first, then what works, then what finished unread`() {
        val s = state(
            thread("done", AgentThreadState.DONE, updatedMs = 9_000L, unread = true),
            thread("read", AgentThreadState.DONE, updatedMs = 9_500L, unread = false),
            thread("work", AgentThreadState.WORKING, updatedMs = 8_000L),
            thread("late", AgentThreadState.WAITING, updatedMs = 7_000L, asks = 1u),
            thread("early", AgentThreadState.WAITING, updatedMs = 7_500L, asks = 1u),
            thread("gone", AgentThreadState.WORKING, archived = true),
            asks = listOf(ask("a1", "late", 60_000L), ask("a2", "early", 30_000L)),
        )
        val part = AgentsSurfaces.of(pc, "Kilele", s, online = true, nowMs = 1L)!!
        assertEquals(listOf("early", "late", "work", "done"), part.rows.map { it.threadId })
        assertEquals(2, part.needsYou)
        assertEquals(1, part.working)
    }

    @Test
    fun `the widget shows three rows at most`() {
        val s = state(*(1..6).map { thread("w$it", AgentThreadState.WORKING, updatedMs = it.toLong()) }.toTypedArray())
        val part = AgentsSurfaces.of(pc, "Kilele", s, online = true, nowMs = 1L)!!
        assertEquals(AgentsSurfaces.MAX_ROWS, part.rows.size)
        val widget = AgentsSurfaces.widget(AgentsSurface(listOf(part)))
        assertEquals(3, widget.rows.size)
        assertEquals("Kilele", widget.pcName)
        assertFalse(widget.offline)
        assertEquals(6, part.working)
    }

    @Test
    fun `the tile says what needs you, what works, nothing, or that the PC can't be reached`() {
        fun pcOf(needs: Int, working: Int, online: Boolean = true, rows: List<SurfaceRow> = emptyList()) =
            AgentsSurface(listOf(SurfacePc(pc, "Kilele", online, needs, working, rows, 1L)))
        val waitingRow = SurfaceRow("t9", "Fix M-Pesa callback retries", "Claude", "duka-pos", SurfaceState.NEEDS_YOU)

        val needs = AgentsSurfaces.tile(pcOf(1, 2, rows = listOf(waitingRow)))
        assertEquals(TileLine.NeedsYou(1), needs.line)
        assertTrue(needs.active)
        assertEquals("t9", needs.threadId)

        val working = AgentsSurfaces.tile(pcOf(0, 2))
        assertEquals(TileLine.Working(2), working.line)
        assertFalse(working.active)
        assertNull(working.threadId)
        assertEquals(pc, working.pcId)

        assertEquals(TileLine.Nothing, AgentsSurfaces.tile(pcOf(0, 0)).line)
        assertEquals(TileLine.Nothing, AgentsSurfaces.tile(AgentsSurface()).line)
        assertNull(AgentsSurfaces.tile(AgentsSurface()).pcId)

        val away = AgentsSurfaces.tile(pcOf(1, 0, online = false, rows = listOf(waitingRow)))
        assertEquals(TileLine.Unreachable("Kilele"), away.line)
        assertFalse(away.active)
    }

    @Test
    fun `an unreachable PC keeps its last rows, dimmed, and a reachable one is shown first`() {
        val s = state(thread("t1", AgentThreadState.WORKING))
        val kilele = AgentsSurfaces.of(pc, "Kilele", s, online = true, nowMs = 10L)!!
        val other = AgentsSurfaces.of("pc2", "Ofisi", state(), online = true, nowMs = 5L)!!
        var surface = AgentsSurfaces.with(AgentsSurfaces.with(AgentsSurface(), other), kilele)
        assertEquals(pc, AgentsSurfaces.primary(surface)!!.pcId)

        surface = AgentsSurfaces.offline(surface, pc)
        assertEquals("pc2", AgentsSurfaces.primary(surface)!!.pcId)
        surface = AgentsSurfaces.without(surface, "pc2")
        val widget = AgentsSurfaces.widget(surface)
        assertTrue(widget.offline)
        assertEquals(listOf("t1"), widget.rows.map { it.threadId })
        assertTrue(AgentsSurfaces.allOffline(AgentsSurfaces.with(surface, kilele)).pcs.none { it.online })
    }

    @Test
    fun `what's read back is bounded and odd text survives`() {
        val row = SurfaceRow("t1", "Tabs\tand\nlines \\ slashes", "Claude", "duka-pos", SurfaceState.DONE)
        val part = SurfacePc(pc, "Kilele", true, 0, 0, listOf(row), 3L)
        val back = AgentsSurfaces.decode(AgentsSurfaces.encode(AgentsSurface(listOf(part))))
        // The separators survive the line format; control characters read back as spaces.
        assertEquals("Tabs and lines \\ slashes", back.pcs.single().rows.single().title)
        assertEquals(listOf("Kilele"), back.pcs.map { it.pcName })

        assertEquals(AgentsSurface(), AgentsSurfaces.decode(null))
        assertEquals(AgentsSurface(), AgentsSurfaces.decode("something else\nP\tpc1"))
        val tooMany = buildString {
            append("agents-surface 1\n")
            repeat(9) { append("P\tpc$it\tPC\t1\t0\t0\t1\n") }
            append("R\tt1\ttitle\tClaude\tf\tWORKING\n".repeat(9))
            append("R\tBAD ID\ttitle\tClaude\tf\tWORKING\n")
            append("R\tt2\ttitle\tClaude\tf\tSOMETHING\n")
        }
        val read = AgentsSurfaces.decode(tooMany)
        assertEquals(AgentsSurfaces.MAX_PCS, read.pcs.size)
        assertEquals(AgentsSurfaces.MAX_ROWS, read.pcs.last().rows.size)
    }

    @Test
    fun `the snapshot is written at most once every 5 s, changes meanwhile riding along`() {
        val throttle = SurfaceThrottle(5_000L)
        assertEquals(0L, throttle.changed(1_000L))
        throttle.wrote(1_000L)
        assertEquals(3_000L, throttle.changed(3_000L))
        // A write already waits: this change goes with it.
        assertNull(throttle.changed(4_000L))
        throttle.wrote(6_000L)
        assertEquals(0L, throttle.changed(11_000L))
    }
}
