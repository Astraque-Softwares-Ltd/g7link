// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.home

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** Home's tiles and pairs keep every name on one line: fewer to a row, or one under another, instead. */
class HomeTilesLayoutTest {

    // A Pixel 8 Pro: 384 dp wide at 2.625 px a dp, less Home's 12 dp each side.
    private val width = 945
    private val gap = 21
    private val inset = 21

    @Test
    fun `names that fit a quarter keep four tiles to a row`() {
        // "Microphone", Home's widest English name, is 65 dp.
        assertEquals(4, homeTileColumns(width, gap, inset, widest = 171))
    }

    @Test
    fun `a name too wide for a quarter drops every tile to three, then two`() {
        // "Schreibtischmodus" is 104 dp, "Área de transferência" 116 dp.
        assertEquals(3, homeTileColumns(width, gap, inset, widest = 273))
        assertEquals(2, homeTileColumns(width, gap, inset, widest = 305))
    }

    @Test
    fun `a row holds one tile at the least`() {
        assertEquals(1, homeTileColumns(width, gap, inset, widest = 2_000))
        assertEquals(1, homeTileColumns(width = 0, gap = gap, inset = inset, widest = 10))
    }

    @Test
    fun `a name exactly as wide as a quarter still fits`() {
        val quarter = (width - gap * 3) / 4 - inset
        assertEquals(4, homeTileColumns(width, gap, inset, widest = quarter))
        assertEquals(3, homeTileColumns(width, gap, inset, widest = quarter + 1))
    }

    @Test
    fun `a pair goes side by side only when both fit on one line`() {
        val cell = homeFitCell(width, gap, count = 2)
        assertEquals((width - gap) / 2, cell)
        assertTrue(homeFitsSideBySide(cell, listOf(cell, cell - 40)))
        // "Phone audio on guchu" was 8 dp too wide beside "Phone as speaker".
        assertFalse(homeFitsSideBySide(cell, listOf(cell - 40, cell + 21)))
    }

    @Test
    fun `one item has the whole width`() {
        assertEquals(width, homeFitCell(width, gap, count = 1))
        assertEquals(width, homeFitCell(width, gap, count = 0))
        assertTrue(homeFitsSideBySide(width, emptyList()))
    }
}
