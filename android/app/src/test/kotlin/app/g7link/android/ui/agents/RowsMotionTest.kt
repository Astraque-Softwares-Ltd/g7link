// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import app.g7link.core.AgentFileStat
import app.g7link.core.AgentRow
import app.g7link.core.AgentRowKind
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The transcript's rows move only while what the reader opened or closed settles. */
class RowsMotionTest {
    private fun row(key: String, kind: AgentRowKind) = AgentRow(key = key, kind = kind, turn = 1u, depth = 0u, itemId = "", seq = 0uL)

    private fun fold(open: Boolean) = row("f:1", AgentRowKind.Fold(workedMs = 134_000L, hidden = 10u, open = open))

    @Test
    fun `nothing tapped, nothing moves`() {
        val still = RowsMotion()
        assertFalse(still.moving)
        assertNull("rows streaming in or Earlier's rows don't open it", still.landed(open = true, nowMs = 1_000L))
    }

    @Test
    fun `a tap moves the rows until its own rows have settled`() {
        val tapped = RowsMotion().tapped("f:1", open = false, nowMs = 1_000L)
        assertTrue(tapped.moving)
        assertEquals(1_000L + RowsMotion.WAIT_MS, tapped.untilMs)
        val landed = requireNotNull(tapped.landed(open = true, nowMs = 1_040L))
        assertTrue(landed.moving)
        assertEquals(1_040L + RowsMotion.SETTLE_MS, landed.untilMs)
        // Rows after the tap's own (an agent's next step) aren't the tap's.
        assertNull(landed.landed(open = false, nowMs = 1_100L))
    }

    @Test
    fun `rows that come before the tap's own don't settle it`() {
        val tapped = RowsMotion().tapped("f:1", open = false, nowMs = 1_000L)
        // Made before the tap reached the rows: the fold is still closed.
        assertNull(tapped.landed(open = false, nowMs = 1_020L))
        assertNotNull(tapped.landed(open = true, nowMs = 1_060L))
    }

    @Test
    fun `closing settles the same way`() {
        val tapped = RowsMotion().tapped("f:1", open = true, nowMs = 5_000L)
        assertEquals(5_030L + RowsMotion.SETTLE_MS, tapped.landed(open = false, nowMs = 5_030L)?.untilMs)
    }

    @Test
    fun `rows too late for the window are some other change`() {
        val tapped = RowsMotion().tapped("f:1", open = false, nowMs = 1_000L)
        assertNull(tapped.landed(open = true, nowMs = 1_000L + RowsMotion.WAIT_MS))
    }

    @Test
    fun `slower animations keep it open longer, never shorter`() {
        val tapped = RowsMotion().tapped("f:1", open = false, nowMs = 1_000L)
        assertEquals(1_000L + 2 * RowsMotion.SETTLE_MS, tapped.landed(open = true, nowMs = 1_000L, scale = 2f)?.untilMs)
        assertEquals(1_000L + RowsMotion.SETTLE_MS, tapped.landed(open = true, nowMs = 1_000L, scale = 0.5f)?.untilMs)
    }

    @Test
    fun `a row that wasn't found when tapped settles on the next rows`() {
        val tapped = RowsMotion().tapped("gone", open = null, nowMs = 1_000L)
        assertNotNull(tapped.landed(open = null, nowMs = 1_030L))
    }

    @Test
    fun `a tapped row is found by its key, a changed file by its Changes row and index`() {
        val thought = row("t:4", AgentRowKind.Thought(preview = "", count = 1u, hasText = true, open = true))
        val files = listOf(
            AgentFileStat(path = "src/a.kt", adds = 3u, dels = 1u, open = false),
            AgentFileStat(path = "src/b.kt", adds = 2u, dels = 0u, open = true),
        )
        val changes = row("c:1", AgentRowKind.Changes(files = files, adds = 5u, dels = 1u, more = 0u))
        val note = row("n:1", AgentRowKind.Notice(text = "Rewound"))
        val rows = listOf(fold(open = false), thought, changes, note)
        assertEquals(false, RowsMotion.openIn(rows, "f:1"))
        assertEquals(true, RowsMotion.openIn(rows, "t:4"))
        assertEquals(false, RowsMotion.openIn(rows, "c:1:0"))
        assertEquals(true, RowsMotion.openIn(rows, "c:1:1"))
        assertNull("no such file", RowsMotion.openIn(rows, "c:1:2"))
        assertNull("a notice doesn't open", RowsMotion.openIn(rows, "n:1"))
        assertNull("not in the rows", RowsMotion.openIn(rows, "f:9"))
        assertNull(RowsMotion.openIn(rows, "plain"))
        assertEquals(true, RowsMotion.openIn(listOf(fold(open = true)), "f:1"))
    }
}
