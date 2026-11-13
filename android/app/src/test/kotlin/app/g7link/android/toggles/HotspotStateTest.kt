// SPDX-License-Identifier: GPL-3.0-or-later
package app.g7link.android.toggles

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class HotspotStateTest {
    @Test
    fun onlyASettledHotspotStateCounts() {
        assertEquals(true, PhoneToggles.hotspotApState(13))
        assertEquals(false, PhoneToggles.hotspotApState(11))
        // One that failed to start is off.
        assertEquals(false, PhoneToggles.hotspotApState(14))
        // Turning on or off, or no state at all: what was known stands.
        assertNull(PhoneToggles.hotspotApState(10))
        assertNull(PhoneToggles.hotspotApState(12))
        assertNull(PhoneToggles.hotspotApState(-1))
    }
}
