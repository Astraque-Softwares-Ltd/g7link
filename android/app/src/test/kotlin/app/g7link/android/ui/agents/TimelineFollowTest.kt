// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.ui.agents

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** The transcript's way back to its end: shown while the reader is up the list, marked when rows came in below. */
class TimelineFollowTest {
    @Test
    fun `the button shows only while the reader is up the list and there is more below`() {
        assertTrue(TimelineFollow.jumpShown(following = false, moreBelow = true))
        // At the end, or following it (rows still landing), nothing to go back to.
        assertFalse(TimelineFollow.jumpShown(following = false, moreBelow = false))
        assertFalse(TimelineFollow.jumpShown(following = true, moreBelow = true))
        assertFalse(TimelineFollow.jumpShown(following = true, moreBelow = false))
    }

    @Test
    fun `rows that come in below while reading up the list are unseen until the reader follows again`() {
        var unseen = TimelineFollow.unseen(was = false, following = false, before = "row:1", newest = "row:2")
        assertTrue(unseen)
        // A row changing in place (the same newest key) or earlier rows going in above keep it as it was.
        unseen = TimelineFollow.unseen(was = unseen, following = false, before = "row:2", newest = "row:2")
        assertTrue(unseen)
        assertFalse(TimelineFollow.unseen(was = false, following = false, before = "row:2", newest = "row:2"))
        // Following again (the button, a send, scrolling to the end) clears it.
        assertFalse(TimelineFollow.unseen(was = true, following = true, before = "row:2", newest = "row:3"))
    }

    @Test
    fun `the first rows to load are never unseen`() {
        assertFalse(TimelineFollow.unseen(was = false, following = false, before = null, newest = "row:1"))
        assertFalse(TimelineFollow.unseen(was = false, following = false, before = "row:1", newest = null))
    }
}
