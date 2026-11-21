// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class EarlierAnchorTest {
    // The list: the Earlier row at 0, then the rows. "m1" was the first message on screen, at index 1.
    private val held = EarlierAnchor(key = "m1", index = 1, offset = 48)

    @Test
    fun `earlier rows above it move the held message down by as many`() {
        assertEquals(4, EarlierAnchor.moved(listOf("e1", "e2", "e3", "m1", "m2"), held, earlier = true))
    }

    @Test
    fun `the last page leaves no Earlier row, and the index follows`() {
        assertEquals(3, EarlierAnchor.moved(listOf("e1", "e2", "e3", "m1", "m2"), held, earlier = false))
    }

    @Test
    fun `nothing came in above it, so the list isn't moved`() {
        assertNull(EarlierAnchor.moved(listOf("m1", "m2", "m3"), held, earlier = true))
        assertNull("new rows at the end don't count", EarlierAnchor.moved(listOf("m1", "m2", "m3", "m4"), held, earlier = true))
    }

    @Test
    fun `a held message that's gone moves nothing`() {
        assertNull(EarlierAnchor.moved(listOf("e1", "m2"), held, earlier = true))
    }
}
