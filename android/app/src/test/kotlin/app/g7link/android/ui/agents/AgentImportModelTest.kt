// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import app.g7link.core.AgentFound
import app.g7link.core.AgentFoundList
import app.g7link.core.AgentImportJob
import app.g7link.core.AgentImportProgress
import app.g7link.core.AgentSessionSource
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

/** Bringing in sessions found on the PC (contract §U9.1 "Import", board 18). */
class AgentImportModelTest {
    private fun found(
        id: String,
        folder: String = "~/kazi/duka-pos",
        updatedMs: Long = 1_000L,
        folderId: String? = "f1",
        open: Boolean = false,
        thread: String? = null,
        damaged: Boolean = false,
    ) = AgentFound(
        id = id,
        agent = "claude",
        title = "Session $id",
        folder = folder,
        folderId = folderId,
        firstMs = null,
        updatedMs = updatedMs,
        bytes = 1_024uL,
        packed = false,
        source = AgentSessionSource.TERMINAL,
        open = open,
        thread = thread,
        damaged = damaged,
    )

    private fun list(vararg found: AgentFound, scanning: Boolean = false, nextMs: Long? = null) =
        AgentFoundList(found = found.toList(), nextMs = nextMs, total = found.size.toUInt(), scanning = scanning, partial = false)

    @Test
    fun `each row is badged as it stands, and only those that can be brought in can be chosen`() {
        assertNull(AgentImport.badge(found("a")))
        assertEquals(FoundBadge.BROUGHT_IN, AgentImport.badge(found("a", thread = "t1")))
        assertEquals(FoundBadge.DAMAGED, AgentImport.badge(found("a", damaged = true)))
        assertEquals(FoundBadge.OPEN, AgentImport.badge(found("a", open = true)))
        assertEquals(FoundBadge.OUTSIDE, AgentImport.badge(found("a", folderId = null)))
        assertTrue(AgentImport.selectable(found("a", open = true)))
        assertTrue("outside the folders: it comes in read only", AgentImport.selectable(found("a", folderId = null)))
        assertFalse(AgentImport.selectable(found("a", thread = "t1")))
        assertFalse(AgentImport.selectable(found("a", damaged = true)))
    }

    @Test
    fun `sessions group by folder, the folder with the newest first, each newest first`() {
        val groups = AgentImport.groups(
            listOf(
                found("old", folder = "~/zamani/ankara", updatedMs = 5L),
                found("a1", updatedMs = 10L),
                found("b1", folder = "~/kazi/shamba-api", updatedMs = 30L),
                found("a2", updatedMs = 20L),
            ),
        )
        assertEquals(listOf("~/kazi/shamba-api", "~/kazi/duka-pos", "~/zamani/ankara"), groups.map { it.folder })
        assertEquals(listOf("a2", "a1"), groups[1].rows.map { it.id })
    }

    @Test
    fun `a first page replaces the list, a later one adds to it, and choices that can't stand go`() {
        var state = AgentImport.listed(ImportState(), list(found("a"), found("b"), scanning = true, nextMs = 5L), more = false)
        assertEquals(ImportLoad.READY, state.load)
        assertTrue(state.scanning)
        assertEquals(5L, state.nextMs)
        state = AgentImport.toggled(AgentImport.toggled(state, "a"), "b")
        state = AgentImport.listed(state, list(found("c"), found("a")), more = true)
        assertEquals(listOf("a", "b", "c"), state.found.map { it.id })
        // "b" was brought in meanwhile.
        state = AgentImport.listed(state, list(found("a"), found("b", thread = "t9")), more = false)
        assertEquals(setOf("a"), state.selected)
    }

    @Test
    fun `choosing is capped at 50 and skips what can't be brought in`() {
        val many = (1..60).map { found("s$it") } + found("in", thread = "t1")
        var state = AgentImport.listed(ImportState(), list(*many.toTypedArray()), more = false)
        for (i in 1..60) state = AgentImport.toggled(state, "s$i")
        assertEquals(AgentImport.MAX_PER_JOB, state.selected.size)
        assertSame(state, AgentImport.toggled(state, "in"))
        state = AgentImport.toggled(state, "s1")
        assertFalse("a second tap unchooses", "s1" in state.selected)
        assertSame("unknown ids do nothing", state, AgentImport.toggled(state, "nope"))
    }

    @Test
    fun `a folder's head chooses what can be chosen in it, then unchooses it`() {
        var state = AgentImport.listed(
            ImportState(),
            list(found("a"), found("b"), found("bad", damaged = true), found("other", folder = "~/x")),
            more = false,
        )
        state = AgentImport.toggledFolder(state, "~/kazi/duka-pos")
        assertEquals(setOf("a", "b"), state.selected)
        state = AgentImport.toggledFolder(state, "~/kazi/duka-pos")
        assertTrue(state.selected.isEmpty())
    }

    @Test
    fun `a run counts what the PC says for its own job, and names those that couldn't be read`() {
        var state = AgentImport.listed(ImportState(), list(found("a"), found("b"), found("c")), more = false)
        state = AgentImport.toggled(AgentImport.toggled(AgentImport.toggled(state, "a"), "b"), "c")
        state = AgentImport.started(state, state.selected.toList(), AgentImportJob("job1", 3u))
        assertTrue(state.busy)
        assertTrue("chosen ones go with the job", state.selected.isEmpty())
        assertSame("nothing is chosen while it runs", state, AgentImport.toggled(state, "a"))
        assertSame(state, AgentImport.progressed(state, AgentImportProgress("other", 3u, 0u, 3u, null)))
        state = AgentImport.progressed(state, AgentImportProgress("job1", 1u, 0u, 3u, "t1"))
        assertEquals(1, state.run!!.done)
        state = AgentImport.progressed(state, AgentImportProgress("job1", 2u, 1u, 3u, "t2"))
        assertTrue(state.run!!.finished)
        assertFalse(state.busy)
        // The list again: "a" and "b" became threads, "c" couldn't be read.
        state = AgentImport.settled(AgentImport.listed(state, list(found("a", thread = "t1"), found("b", thread = "t2"), found("c")), more = false))
        assertEquals(listOf("c"), state.run!!.failures.map { it.id })
    }

    @Test
    fun `cancel keeps what was brought in and ends the run`() {
        var state = AgentImport.started(ImportState(), listOf("a", "b", "c", "d"), AgentImportJob("j", 4u))
        state = AgentImport.progressed(state, AgentImportProgress("j", 1u, 0u, 4u, "t1"))
        state = AgentImport.cancelled(state)
        assertTrue(state.run!!.finished)
        assertEquals(1, state.run!!.total)
    }
}
