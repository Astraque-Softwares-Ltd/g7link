// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.mirror

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PhoneLockTest {
    @Test
    fun `a locked phone takes no input, and says so once per session per lock`() {
        val gate = PhoneLock.Gate()
        assertEquals(PhoneLock.Input.Pass, gate.check(locked = false, pc = "pc", session = 0u))
        assertEquals(PhoneLock.Input.DropAndTell, gate.check(locked = true, pc = "pc", session = 0u))
        assertEquals(PhoneLock.Input.Drop, gate.check(locked = true, pc = "pc", session = 0u))
        assertEquals(PhoneLock.Input.DropAndTell, gate.check(locked = true, pc = "pc", session = 3u))
        assertEquals(PhoneLock.Input.DropAndTell, gate.check(locked = true, pc = "other", session = 0u))
        // Unlocked, then locked again: told again.
        assertEquals(PhoneLock.Input.Pass, gate.check(locked = false, pc = "pc", session = 0u))
        assertEquals(PhoneLock.Input.DropAndTell, gate.check(locked = true, pc = "pc", session = 0u))
    }

    @Test
    fun `mirroring starts without a tap only through Elevated on an unlocked phone`() {
        assertTrue(PhoneLock.startsWithoutTap(elevatedRunning = true, locked = false, soundOnly = false))
        assertFalse(PhoneLock.startsWithoutTap(elevatedRunning = true, locked = true, soundOnly = false))
        assertFalse(PhoneLock.startsWithoutTap(elevatedRunning = false, locked = false, soundOnly = false))
        // The sound alone isn't control.
        assertTrue(PhoneLock.startsWithoutTap(elevatedRunning = true, locked = true, soundOnly = true))
    }
}
